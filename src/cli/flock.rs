//! `flockr flock ...`: start a flock, invite and join machines, list and
//! remove members.
//!
//! These commands work on the files in `state_dir()/flock/`. The server's
//! flock node notices changes on its next poll; `join` connects to the
//! inviting machine itself, so the joining machine's server need not run.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde_json::{json, Value};

use crate::flock::channel::Channel;
use crate::flock::invite::{self, Decision, InvitePayload};
use crate::flock::keys::{self, encode_key, MemberKeys};
use crate::flock::log::{self, Entry, Kind, MemberInfo, Op, Role, State};
use crate::flock::owner;
use crate::flock::store;
use crate::mobile::store::now_unix;

const POLL: Duration = Duration::from_millis(500);
/// How long `invite` waits for the joining machine to receive the result.
const HANDOFF_WAIT: Duration = Duration::from_secs(30);

pub(super) fn run_flock_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("init") => init(&args[1..]),
        Some("invite") => invite(&args[1..]),
        Some("join") => join(&args[1..]),
        Some("members") => members(&args[1..]),
        Some("status") => status(&args[1..]),
        Some("remove") => remove(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_help();
            Ok(0)
        }
        _ => {
            print_help();
            Ok(2)
        }
    }
}

fn print_help() {
    eprintln!("flockr flock commands:");
    eprintln!("  flockr flock init [--name NAME]                 start a flock; this machine holds the owner key");
    eprintln!(
        "  flockr flock invite [--role ROLE] [--ttl SECS]  invite a machine (owner machine only)"
    );
    eprintln!("  flockr flock join <url>                         join with an invite URL");
    eprintln!("  flockr flock members [--json]                   list members");
    eprintln!("  flockr flock status [--json]                    show this machine's flock state");
    eprintln!("  flockr flock remove <member-id|name>            remove a member everywhere");
    eprintln!();
    eprintln!("Roles: admin (default for invites), member, viewer.");
}

fn config() -> crate::config::FlockConfig {
    crate::config::Config::load().config.flock
}

fn configured_addrs(config: &crate::config::FlockConfig) -> Vec<String> {
    config
        .listen_addrs()
        .0
        .iter()
        .map(std::net::SocketAddr::to_string)
        .collect()
}

fn load_state(dir: &Path) -> std::io::Result<Option<State>> {
    let state = store::load_state(dir)?;
    if state.is_none() {
        eprintln!(
            "this machine is not in a flock; run `flockr flock init` or `flockr flock join <url>`"
        );
    }
    Ok(state)
}

fn node_running(dir: &Path) -> Option<store::NodeRecord> {
    store::load_node(dir).filter(|record| crate::platform::process_exists(record.pid))
}

fn warn_if_node_off(config: &crate::config::FlockConfig, dir: &Path) {
    if !config.enabled || config.listen_addrs().0.is_empty() {
        eprintln!(
            "note: set [flock] enabled = true and listen = [\"<ip>\"] in {} and restart the server so other machines can connect",
            crate::config::config_path().display()
        );
    } else if node_running(dir).is_none() {
        eprintln!("note: the server's flock node is not running (is the server running and restarted since enabling [flock]?)");
    }
}

/// Reads a line from the terminal without echo. Errors when stdin is not a
/// terminal: the owner passphrase is never taken from a pipe or a variable.
fn prompt_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        return Err(std::io::Error::other(
            "the owner passphrase must be typed in a terminal",
        ));
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let mut secret = String::new();
    let result = loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "canceled",
                    ))
                }
                KeyCode::Backspace => {
                    secret.pop();
                }
                KeyCode::Char(ch) => secret.push(ch),
                _ => {}
            },
            Ok(_) => {}
            Err(err) => break Err(err),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result.map(|()| secret)
}

fn prompt_line(prompt: &str) -> std::io::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn unlock_owner(dir: &Path) -> std::io::Result<ed25519_dalek::SigningKey> {
    for _ in 0..3 {
        let passphrase = prompt_secret("Owner passphrase: ")?;
        match owner::unlock(dir, &passphrase) {
            Ok(key) => return Ok(key),
            Err(owner::UnlockError::WrongPassphrase) => eprintln!("wrong passphrase"),
            Err(err) => return Err(std::io::Error::other(err.to_string())),
        }
    }
    Err(std::io::Error::other("too many wrong passphrases"))
}

fn member_info(keys: &MemberKeys, name: String, addrs: Vec<String>) -> std::io::Result<MemberInfo> {
    Ok(MemberInfo {
        id: keys.member_id(),
        name,
        kind: Kind::Computer,
        sign_key: encode_key(&keys.sign_public()),
        noise_key: encode_key(&keys.noise_public()?),
        addrs,
    })
}

fn init(args: &[String]) -> std::io::Result<i32> {
    let config = config();
    let name = match args {
        [] => "flock".to_owned(),
        [flag, name] if flag == "--name" && !name.trim().is_empty() => name.trim().to_owned(),
        _ => {
            eprintln!("usage: flockr flock init [--name NAME]");
            return Ok(2);
        }
    };
    if name.chars().count() > log::MAX_NAME_CHARS {
        eprintln!(
            "error: the name is longer than {} characters",
            log::MAX_NAME_CHARS
        );
        return Ok(2);
    }
    let dir = store::flock_dir();
    if store::load_group(&dir)?.is_some() {
        eprintln!("error: this machine is already in a flock");
        return Ok(1);
    }
    eprintln!("The owner key controls who is in the flock. It stays on this machine,");
    eprintln!("encrypted with a passphrase you type each time you add or remove a member.");
    let passphrase = prompt_secret("New owner passphrase: ")?;
    if passphrase.chars().count() < owner::MIN_PASSPHRASE_CHARS {
        eprintln!(
            "error: use at least {} characters",
            owner::MIN_PASSPHRASE_CHARS
        );
        return Ok(2);
    }
    if prompt_secret("Repeat it: ")? != passphrase {
        eprintln!("error: the passphrases differ");
        return Ok(2);
    }
    let owner = owner::create(&dir, &passphrase)?;
    let keys = keys::load_or_create(&dir)?;
    let now = now_unix();
    let root = log::genesis(&name, &owner, now);
    store::save_entry(&dir, &root)?;
    store::save_group(&dir, &root.hash())?;
    let state = log::fold(std::slice::from_ref(&root), None)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let me = member_info(&keys, config.display_name(), configured_addrs(&config))?;
    store::save_entry(
        &dir,
        &state.append(
            Op::Add {
                member: me.clone(),
                role: Role::Admin,
            },
            &owner,
            now,
        ),
    )?;
    let state = store::load_state(&dir)?.ok_or_else(|| std::io::Error::other("flock vanished"))?;
    println!("Started flock {name:?} ({})", short(&state.group));
    println!(
        "This machine is {} ({}), admin and owner device.",
        me.name, me.id
    );
    warn_if_node_off(&config, &dir);
    Ok(0)
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

fn invite(args: &[String]) -> std::io::Result<i32> {
    let usage = "usage: flockr flock invite [--role admin|member|viewer] [--ttl SECS]";
    let mut role = Role::Admin;
    let mut ttl = invite::DEFAULT_TTL_SECS;
    let mut index = 0;
    while index < args.len() {
        match (args[index].as_str(), args.get(index + 1)) {
            ("--role", Some(value)) => {
                let Some(parsed) = Role::parse(value) else {
                    eprintln!("{usage}");
                    return Ok(2);
                };
                role = parsed;
                index += 1;
            }
            ("--ttl", Some(value)) => {
                let Ok(parsed) = value.parse::<u64>() else {
                    eprintln!("{usage}");
                    return Ok(2);
                };
                ttl = parsed.clamp(60, 3600);
                index += 1;
            }
            _ => {
                eprintln!("{usage}");
                return Ok(2);
            }
        }
        index += 1;
    }
    let dir = store::flock_dir();
    let Some(state) = load_state(&dir)? else {
        return Ok(1);
    };
    if owner::public(&dir).as_deref() != Some(state.owner.as_str()) {
        eprintln!(
            "error: invites are made on the owner device; this machine doesn't hold the owner key"
        );
        return Ok(1);
    }
    let config = config();
    let addrs = configured_addrs(&config);
    if addrs.is_empty() || node_running(&dir).is_none() {
        warn_if_node_off(&config, &dir);
        eprintln!("error: the joining machine connects to this machine's flock node, which isn't reachable");
        return Ok(1);
    }
    let keys = keys::load_or_create(&dir)?;
    let (record, secret) = invite::create(&dir, role, ttl, now_unix())?;
    let payload = InvitePayload {
        v: invite::PAYLOAD_VERSION,
        group: state.group.clone(),
        name: state.name.clone(),
        host: keys.member_id(),
        noise_key: encode_key(&keys.noise_public()?),
        addrs,
        invite: record.id.clone(),
        secret,
        exp: record.expires_at,
    };
    let url = payload.to_url();
    if let Ok(qr) = crate::mobile::pairing::render_qr(&url) {
        println!("{qr}");
    }
    println!("On the new machine, run:");
    println!("  flockr flock join '{url}'");
    println!(
        "Joins as {}. Expires in {}:{:02}.",
        role.as_str(),
        ttl / 60,
        ttl % 60
    );

    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        let _ = ctrlc::set_handler(move || interrupted.store(true, Ordering::Release));
    }
    let request = loop {
        if interrupted.load(Ordering::Acquire) {
            invite::clear(&dir, &record.id);
            eprintln!("Invite canceled");
            return Ok(130);
        }
        if let Some(request) = invite::load_request(&dir, &record.id) {
            break request;
        }
        if now_unix() >= record.expires_at {
            invite::clear(&dir, &record.id);
            println!("Invite expired");
            return Ok(1);
        }
        std::thread::sleep(POLL);
    };
    println!();
    println!(
        "Join request from {:?} ({})",
        request.member.name, request.member.id
    );
    println!(
        "Addresses: {}",
        if request.member.addrs.is_empty() {
            "none".to_owned()
        } else {
            request.member.addrs.join(", ")
        }
    );
    println!("Short code: {}", request.sas);
    let answer =
        prompt_line("Does the new machine show the same short code? Approve the join [y/N]: ")?;
    if !matches!(answer.as_str(), "y" | "Y" | "yes") {
        invite::save_decision(&dir, &record.id, &Decision::Denied)?;
        println!("Join declined");
        return Ok(1);
    }
    let owner_key = unlock_owner(&dir)?;
    let state = store::load_state(&dir)?.ok_or_else(|| std::io::Error::other("flock vanished"))?;
    let entry = state.append(
        Op::Add {
            member: request.member.clone(),
            role: request.role,
        },
        &owner_key,
        now_unix(),
    );
    store::save_entry(&dir, &entry)?;
    let state = store::load_state(&dir)?.ok_or_else(|| std::io::Error::other("flock vanished"))?;
    if let Some((_, reason)) = state
        .rejected
        .iter()
        .find(|(hash, _)| *hash == entry.hash())
    {
        invite::save_decision(&dir, &record.id, &Decision::Denied)?;
        eprintln!("error: the flock refused the new member: {reason}");
        return Ok(1);
    }
    invite::save_decision(&dir, &record.id, &Decision::Approved)?;
    let started = std::time::Instant::now();
    while invite::load_request(&dir, &record.id).is_some() && started.elapsed() < HANDOFF_WAIT {
        std::thread::sleep(POLL);
    }
    println!(
        "Added {} ({}) as {}",
        request.member.name,
        request.member.id,
        request.role.as_str()
    );
    Ok(0)
}

fn join(args: &[String]) -> std::io::Result<i32> {
    let [url] = args else {
        eprintln!("usage: flockr flock join <url>");
        return Ok(2);
    };
    let payload = match InvitePayload::from_url(url) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("error: {err}");
            return Ok(2);
        }
    };
    let dir = store::flock_dir();
    if store::load_group(&dir)?.is_some() {
        eprintln!("error: this machine is already in a flock");
        return Ok(1);
    }
    if payload.exp <= now_unix() {
        eprintln!("error: the invite has expired");
        return Ok(1);
    }
    let Some(host_key) = keys::decode_key(&payload.noise_key) else {
        eprintln!("error: the invite's key is malformed");
        return Ok(2);
    };
    let config = config();
    let keys = keys::load_or_create(&dir)?;
    let me = member_info(&keys, config.display_name(), configured_addrs(&config))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let joined = runtime.block_on(join_over_channel(&payload, &keys, &me, &host_key));
    let entries = match joined {
        Ok(joined) => joined,
        Err(err) => {
            eprintln!("error: {err}");
            return Ok(1);
        }
    };
    let state = log::fold(&entries, Some(&payload.group))
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let Some(member) = state.active(&me.id) else {
        eprintln!("error: the host's log doesn't list this machine");
        return Ok(1);
    };
    if member.info.noise_key != me.noise_key || member.info.sign_key != me.sign_key {
        eprintln!("error: the host's log lists this machine with other keys");
        return Ok(1);
    }
    for entry in &entries {
        store::save_entry(&dir, entry)?;
    }
    store::save_group(&dir, &payload.group)?;
    println!(
        "Joined flock {:?} as {} ({}), {}",
        state.name,
        me.name,
        me.id,
        member.role.as_str()
    );
    warn_if_node_off(&config, &dir);
    Ok(0)
}

async fn join_over_channel(
    payload: &InvitePayload,
    keys: &MemberKeys,
    me: &MemberInfo,
    host_key: &[u8; 32],
) -> Result<Vec<Entry>, String> {
    let mut channel = None;
    let mut last_error = "the invite has no addresses".to_owned();
    for addr in &payload.addrs {
        match Channel::connect(addr, &keys.noise, host_key, &payload.group).await {
            Ok(connected) => {
                channel = Some(connected);
                break;
            }
            Err(err) => last_error = format!("could not reach {addr}: {err}"),
        }
    }
    let mut channel = channel.ok_or(last_error)?;
    let hello = json!({
        "t": "join",
        "invite": payload.invite,
        "secret": payload.secret,
        "name": me.name,
        "sign_key": me.sign_key,
        "addrs": me.addrs,
    });
    channel.send(&hello).await.map_err(|err| err.to_string())?;
    let wait = Duration::from_secs(payload.exp.saturating_sub(now_unix()) + 60);
    let reply = |message: Option<Value>| -> Result<Value, String> {
        let message = message.ok_or("the host closed the connection")?;
        if message["t"] == "error" {
            return Err(message["reason"].as_str().unwrap_or("refused").to_owned());
        }
        Ok(message)
    };
    let first = tokio::time::timeout(Duration::from_secs(15), channel.recv())
        .await
        .map_err(|_| "the host didn't answer".to_owned())?
        .map_err(|err| err.to_string())?;
    let first = reply(first)?;
    if first["t"] != "join.wait" {
        return Err("unexpected reply from the host".to_owned());
    }
    println!("Short code: {}", first["sas"].as_str().unwrap_or("?"));
    println!("Check that the inviting machine shows the same code, then approve the join there.");
    let second = tokio::time::timeout(wait, channel.recv())
        .await
        .map_err(|_| "timed out waiting for approval".to_owned())?
        .map_err(|err| err.to_string())?;
    let second = reply(second)?;
    if second["t"] != "joined" || second["group"] != payload.group.as_str() {
        return Err("unexpected reply from the host".to_owned());
    }
    let entries: Vec<Entry> =
        serde_json::from_value(second["entries"].clone()).map_err(|err| err.to_string())?;
    channel.close().await;
    Ok(entries)
}

fn json_flag(args: &[String], usage: &str) -> Option<bool> {
    match args {
        [] => Some(false),
        [flag] if flag == "--json" => Some(true),
        _ => {
            eprintln!("{usage}");
            None
        }
    }
}

fn members(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_flag(args, "usage: flockr flock members [--json]") else {
        return Ok(2);
    };
    let dir = store::flock_dir();
    let Some(state) = load_state(&dir)? else {
        return Ok(1);
    };
    let me = keys::load_or_create(&dir)?.member_id();
    let node = node_running(&dir);
    let connected = node
        .as_ref()
        .map(|node| node.connected.clone())
        .unwrap_or_default();
    let sync = |id: &str| match &node {
        None => "unknown",
        Some(_) if id == me => "-",
        Some(node) if node.behind.iter().any(|behind| behind == id) => "behind",
        Some(_) => "synced",
    };
    let rows: Vec<Value> = state
        .members
        .values()
        .map(|member| {
            json!({
                "id": member.info.id,
                "name": member.info.name,
                "kind": member.info.kind.as_str(),
                "role": member.role.as_str(),
                "addrs": member.info.addrs,
                "removed": member.removed,
                "this_machine": member.info.id == me,
                "connected": connected.contains(&member.info.id),
                "sync": (!member.removed).then(|| sync(&member.info.id)),
            })
        })
        .collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "members": rows }))?
        );
        return Ok(0);
    }
    println!(
        "{:<20} {:<18} {:<7} {:<10} {:<7} ADDRESSES",
        "ID", "NAME", "ROLE", "STATE", "LOG"
    );
    for member in state.members.values() {
        let status = if member.removed {
            "removed"
        } else if member.info.id == me {
            "this"
        } else if connected.contains(&member.info.id) {
            "connected"
        } else {
            "offline"
        };
        println!(
            "{:<20} {:<18} {:<7} {:<10} {:<7} {}",
            member.info.id,
            member.info.name,
            member.role.as_str(),
            status,
            if member.removed {
                "-"
            } else {
                sync(&member.info.id)
            },
            member.info.addrs.join(", ")
        );
    }
    Ok(0)
}

fn status(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_flag(args, "usage: flockr flock status [--json]") else {
        return Ok(2);
    };
    let dir = store::flock_dir();
    let Some(state) = load_state(&dir)? else {
        return Ok(1);
    };
    let me = keys::load_or_create(&dir)?.member_id();
    let role = state.active(&me).map(|member| member.role.as_str());
    let owner_device = owner::public(&dir).as_deref() == Some(state.owner.as_str());
    let node = node_running(&dir);
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "group": state.group,
                "name": state.name,
                "member": me,
                "role": role,
                "owner_device": owner_device,
                "node": node,
                "rejected_entries": state.rejected.len(),
                "pending_entries": state.pending,
            }))?
        );
        return Ok(0);
    }
    println!("Flock:        {} ({})", state.name, short(&state.group));
    println!(
        "This machine: {me}, {}",
        role.unwrap_or("not an active member")
    );
    println!("Owner device: {}", if owner_device { "yes" } else { "no" });
    match &node {
        Some(node) => println!(
            "Node:         listening on {}; {} peer(s) connected",
            if node.bound.is_empty() {
                "nothing".to_owned()
            } else {
                node.bound.join(", ")
            },
            node.connected.len()
        ),
        None => println!("Node:         not running"),
    }
    if let Some(node) = &node {
        if !node.caught_up {
            println!("Catch-up:     still reaching members after a start or wake");
        }
        for id in &node.behind {
            let name = state
                .members
                .get(id)
                .map(|member| member.info.name.as_str())
                .unwrap_or(id);
            let last = match node.last_sync.get(id) {
                Some(at) => format!("last synced {} ago", ago(now_unix().saturating_sub(*at))),
                None => "not synced since this server started".to_owned(),
            };
            println!("Behind:       {name} lacks recent log entries ({last})");
        }
    }
    if !state.rejected.is_empty() {
        println!("Log:          {} entr(ies) rejected", state.rejected.len());
    }
    if state.pending > 0 {
        println!(
            "Log:          {} entr(ies) waiting for earlier entries",
            state.pending
        );
    }
    Ok(0)
}

fn ago(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn remove(args: &[String]) -> std::io::Result<i32> {
    let [target] = args else {
        eprintln!("usage: flockr flock remove <member-id|name>");
        return Ok(2);
    };
    let dir = store::flock_dir();
    let Some(state) = load_state(&dir)? else {
        return Ok(1);
    };
    let matches: Vec<&log::Member> = state
        .members
        .values()
        .filter(|member| {
            !member.removed && (member.info.id == *target || member.info.name == *target)
        })
        .collect();
    let member = match matches.as_slice() {
        [member] => (*member).clone(),
        [] => {
            eprintln!("error: no active member {target:?}");
            return Ok(1);
        }
        _ => {
            eprintln!("error: several members are named {target:?}; use the member ID");
            return Ok(1);
        }
    };
    let keys = keys::load_or_create(&dir)?;
    let my_role = state.active(&keys.member_id()).map(|member| member.role);
    let entry = if owner::public(&dir).as_deref() == Some(state.owner.as_str()) {
        let owner_key = unlock_owner(&dir)?;
        state.append(
            Op::Remove {
                member: member.info.id.clone(),
            },
            &owner_key,
            now_unix(),
        )
    } else if my_role == Some(Role::Admin) && member.role != Role::Admin {
        state.append(
            Op::Remove {
                member: member.info.id.clone(),
            },
            &keys.sign,
            now_unix(),
        )
    } else {
        eprintln!("error: only the owner device removes admins; admins remove members and viewers");
        return Ok(1);
    };
    store::save_entry(&dir, &entry)?;
    let state = store::load_state(&dir)?.ok_or_else(|| std::io::Error::other("flock vanished"))?;
    if let Some((_, reason)) = state
        .rejected
        .iter()
        .find(|(hash, _)| *hash == entry.hash())
    {
        eprintln!("error: the flock refused the removal: {reason}");
        return Ok(1);
    }
    println!(
        "Removed {} ({}). Connected members drop it now; `flockr flock status` lists members that haven't received the removal yet.",
        member.info.name, member.info.id
    );
    Ok(0)
}
