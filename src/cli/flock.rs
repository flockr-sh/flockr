//! `flockr flock ...`: start a flock with this machine as the hub, invite
//! and join machines, list and remove members.
//!
//! These commands work on the files in `state_dir()/flock/`. The server's
//! flock node notices changes on its next poll; `join` connects to the hub
//! itself, so the joining machine's server need not run.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::flock::channel::Channel;
use crate::flock::invite::{self, Decision, InvitePayload};
use crate::flock::keys::{self, encode_key, MemberKeys};
use crate::flock::store::{self, GroupFile, HubFile, Kind, Member, Role};
use crate::mobile::store::now_unix;

const POLL: Duration = Duration::from_millis(500);
/// How long `invite` waits for the joining machine to receive the result.
const HANDOFF_WAIT: Duration = Duration::from_secs(30);
const NOT_IN_FLOCK: &str =
    "this machine is not in a flock; run `flockr flock init` or `flockr flock join <url>`";
const HUB_ONLY: &str = "error: run this on the hub";

pub(super) fn run_flock_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("init") => init(&args[1..]),
        Some("invite") => invite(&args[1..]),
        Some("join") => join(&args[1..]),
        Some("members") => members(&args[1..]),
        Some("agents") => agents(&args[1..]),
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
    eprintln!("  flockr flock init [--name NAME]                 start a flock with this machine as the hub");
    eprintln!("  flockr flock invite [--role ROLE] [--ttl SECS]  invite a machine (on the hub)");
    eprintln!("  flockr flock join <url>                         join with an invite URL");
    eprintln!("  flockr flock members [--json]                   list members");
    eprintln!("  flockr flock agents [--json]                    list agents on every member");
    eprintln!("  flockr flock status [--json]                    show this machine's flock state");
    eprintln!("  flockr flock remove <member-id|name>            remove a member (on the hub)");
    eprintln!();
    eprintln!();
    eprintln!("Run any API command on a member with `flockr --machine <member> <command>`.");
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

fn node_running(dir: &Path) -> Option<store::NodeRecord> {
    store::load_node(dir).filter(|record| crate::platform::process_exists(record.pid))
}

fn warn_if_node_off(config: &crate::config::FlockConfig, dir: &Path, hub: bool) {
    if !config.enabled || (hub && config.listen_addrs().0.is_empty()) {
        let listen = if hub { " and listen = [\"<ip>\"]" } else { "" };
        eprintln!(
            "note: set [flock] enabled = true{listen} in {} and restart the server",
            crate::config::config_path().display()
        );
    } else if node_running(dir).is_none() {
        eprintln!("note: the server's flock node is not running (is the server running and restarted since enabling [flock]?)");
    }
}

fn prompt_line(prompt: &str) -> std::io::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
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
    if name.chars().count() > store::MAX_NAME_CHARS {
        eprintln!(
            "error: the name is longer than {} characters",
            store::MAX_NAME_CHARS
        );
        return Ok(2);
    }
    let dir = store::flock_dir();
    let keys = keys::load_or_create(&dir)?;
    let me = Member {
        id: keys::new_id(store::MEMBER_ID_PREFIX)?,
        name: config.display_name(),
        kind: Kind::Computer,
        role: Role::Admin,
        noise_key: encode_key(&keys.noise_public()?),
        joined_at: now_unix(),
    };
    let hub = HubFile {
        version: 1,
        group: keys::new_id(store::GROUP_ID_PREFIX)?,
        name: name.clone(),
        hub: me.id.clone(),
        members: vec![me.clone()],
    };
    if let Err(err) = store::create_hub(&dir, &hub) {
        eprintln!("error: {err}");
        return Ok(1);
    }
    println!("Started flock {name:?} ({})", hub.group);
    println!("This machine is the hub: {} ({}).", me.name, me.id);
    warn_if_node_off(&config, &dir, true);
    Ok(0)
}

fn load_hub(dir: &Path) -> std::io::Result<Option<HubFile>> {
    let hub = store::load_hub(dir)?;
    if hub.is_none() {
        if store::load_group(dir)?.is_some() {
            eprintln!("{HUB_ONLY}");
        } else {
            eprintln!("{NOT_IN_FLOCK}");
        }
    }
    Ok(hub)
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
    let Some(hub) = load_hub(&dir)? else {
        return Ok(1);
    };
    let config = config();
    let addrs = configured_addrs(&config);
    if addrs.is_empty() || node_running(&dir).is_none() {
        warn_if_node_off(&config, &dir, true);
        eprintln!("error: the joining machine connects to this machine's flock node, which isn't reachable");
        return Ok(1);
    }
    let keys = keys::load_or_create(&dir)?;
    let (record, secret) = invite::create(&dir, role, ttl, now_unix())?;
    let payload = InvitePayload {
        v: invite::PAYLOAD_VERSION,
        group: hub.group.clone(),
        name: hub.name.clone(),
        hub: hub.hub.clone(),
        hub_key: encode_key(&keys.noise_public()?),
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
    println!("Join request from {:?}", request.name);
    println!("Short code: {}", request.sas);
    let answer =
        prompt_line("Does the new machine show the same short code? Approve the join [y/N]: ")?;
    if !matches!(answer.as_str(), "y" | "Y" | "yes") {
        invite::save_decision(&dir, &record.id, &Decision::Denied)?;
        println!("Join declined");
        return Ok(1);
    }
    let member = Member {
        id: keys::new_id(store::MEMBER_ID_PREFIX)?,
        name: request.name.clone(),
        kind: request.kind,
        role: request.role,
        noise_key: request.noise_key.clone(),
        joined_at: now_unix(),
    };
    let added = store::update_hub(&dir, |hub| {
        if hub.by_noise_key(&member.noise_key).is_some() {
            return false;
        }
        hub.members.push(member.clone());
        true
    })?;
    if !added {
        invite::save_decision(&dir, &record.id, &Decision::Denied)?;
        eprintln!("error: that machine is already a member");
        return Ok(1);
    }
    invite::save_decision(&dir, &record.id, &Decision::Approved)?;
    let started = std::time::Instant::now();
    while invite::load_request(&dir, &record.id).is_some() && started.elapsed() < HANDOFF_WAIT {
        std::thread::sleep(POLL);
    }
    println!(
        "Added {} ({}) as {}",
        member.name,
        member.id,
        member.role.as_str()
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
    if store::load_hub(&dir)?.is_some() || store::load_group(&dir)?.is_some() {
        eprintln!("error: this machine is already in a flock");
        return Ok(1);
    }
    if payload.exp <= now_unix() {
        eprintln!("error: the invite has expired");
        return Ok(1);
    }
    let Some(hub_key) = keys::decode_key(&payload.hub_key) else {
        eprintln!("error: the invite's key is malformed");
        return Ok(2);
    };
    let config = config();
    let keys = keys::load_or_create(&dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let joined = runtime.block_on(join_over_channel(
        &payload,
        &keys,
        &config.display_name(),
        &hub_key,
    ));
    let joined = match joined {
        Ok(joined) => joined,
        Err(err) => {
            eprintln!("error: {err}");
            return Ok(1);
        }
    };
    let Some(member) = joined["member"].as_str() else {
        eprintln!("error: the hub didn't send a member ID");
        return Ok(1);
    };
    store::save_group(
        &dir,
        &GroupFile {
            version: 1,
            group: payload.group.clone(),
            name: payload.name.clone(),
            member: member.to_owned(),
            hub: payload.hub.clone(),
            hub_key: payload.hub_key.clone(),
            hub_addrs: payload.addrs.clone(),
        },
    )?;
    println!(
        "Joined flock {:?} as {} ({member}), {}",
        payload.name,
        config.display_name(),
        joined["role"].as_str().unwrap_or("?")
    );
    warn_if_node_off(&config, &dir, false);
    Ok(0)
}

async fn join_over_channel(
    payload: &InvitePayload,
    keys: &MemberKeys,
    name: &str,
    hub_key: &[u8; 32],
) -> Result<Value, String> {
    let mut channel = None;
    let mut last_error = "the invite has no addresses".to_owned();
    for addr in &payload.addrs {
        match Channel::connect(addr, &keys.noise, hub_key, &payload.group).await {
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
        "name": name,
    });
    channel.send(&hello).await.map_err(|err| err.to_string())?;
    let wait = Duration::from_secs(payload.exp.saturating_sub(now_unix()) + 60);
    let reply = |message: Option<Value>| -> Result<Value, String> {
        let message = message.ok_or("the hub closed the connection")?;
        if message["t"] == "error" {
            return Err(message["reason"].as_str().unwrap_or("refused").to_owned());
        }
        Ok(message)
    };
    let first = tokio::time::timeout(Duration::from_secs(15), channel.recv())
        .await
        .map_err(|_| "the hub didn't answer".to_owned())?
        .map_err(|err| err.to_string())?;
    let first = reply(first)?;
    if first["t"] != "join.wait" {
        return Err("unexpected reply from the hub".to_owned());
    }
    println!("Short code: {}", first["sas"].as_str().unwrap_or("?"));
    println!("Check that the hub shows the same code, then approve the join there.");
    let second = tokio::time::timeout(wait, channel.recv())
        .await
        .map_err(|_| "timed out waiting for approval".to_owned())?
        .map_err(|err| err.to_string())?;
    let second = reply(second)?;
    if second["t"] != "joined"
        || second["group"] != payload.group.as_str()
        || second["hub"] != payload.hub.as_str()
    {
        return Err("unexpected reply from the hub".to_owned());
    }
    channel.close().await;
    Ok(second)
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

/// The member list and who is connected, from the hub's files on the hub
/// and from the hub's last update elsewhere.
struct Roster {
    members: Vec<Member>,
    connected: Option<Vec<String>>,
    hub: String,
    me: String,
    /// Unix time of the hub's last update, on members.
    as_of: Option<u64>,
}

fn roster(dir: &Path) -> std::io::Result<Option<Roster>> {
    let node = node_running(dir);
    if let Some(hub) = store::load_hub(dir)? {
        return Ok(Some(Roster {
            members: hub.members,
            connected: node.map(|node| node.connected),
            me: hub.hub.clone(),
            hub: hub.hub,
            as_of: None,
        }));
    }
    let Some(group) = store::load_group(dir)? else {
        eprintln!("{NOT_IN_FLOCK}");
        return Ok(None);
    };
    let cache = store::load_members_cache(dir).unwrap_or_default();
    let hub_up = node.is_some_and(|node| node.connected.contains(&group.hub));
    Ok(Some(Roster {
        members: cache.members,
        connected: hub_up.then_some(cache.connected),
        hub: group.hub,
        me: group.member,
        as_of: (cache.updated_at > 0).then_some(cache.updated_at),
    }))
}

fn members(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_flag(args, "usage: flockr flock members [--json]") else {
        return Ok(2);
    };
    let dir = store::flock_dir();
    let Some(roster) = roster(&dir)? else {
        return Ok(1);
    };
    let state = |member: &Member| {
        if member.id == roster.me {
            "this"
        } else {
            match &roster.connected {
                None => "unknown",
                Some(connected) if connected.contains(&member.id) => "connected",
                Some(_) => "offline",
            }
        }
    };
    if as_json {
        let rows: Vec<Value> = roster
            .members
            .iter()
            .map(|member| {
                json!({
                    "id": member.id,
                    "name": member.name,
                    "kind": member.kind.as_str(),
                    "role": member.role.as_str(),
                    "hub": member.id == roster.hub,
                    "this_machine": member.id == roster.me,
                    "state": state(member),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "members": rows, "as_of": roster.as_of }))?
        );
        return Ok(0);
    }
    if roster.members.is_empty() {
        println!("No member list yet; it arrives when this machine connects to the hub.");
        return Ok(0);
    }
    println!("{:<20} {:<18} {:<7} STATE", "ID", "NAME", "ROLE");
    for member in &roster.members {
        let hub = if member.id == roster.hub { ", hub" } else { "" };
        println!(
            "{:<20} {:<18} {:<7} {}{hub}",
            member.id,
            member.name,
            member.role.as_str(),
            state(member)
        );
    }
    if roster.connected.is_none() {
        if let Some(at) = roster.as_of {
            println!(
                "Not connected to the hub; list as of {} ago.",
                ago(now_unix().saturating_sub(at))
            );
        }
    }
    Ok(0)
}

fn agents(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_flag(args, "usage: flockr flock agents [--json]") else {
        return Ok(2);
    };
    let dir = store::flock_dir();
    let Some((members, _)) = crate::flock::client::known_members(&dir)? else {
        eprintln!("{NOT_IN_FLOCK}");
        return Ok(1);
    };
    let ids: Vec<String> = members.iter().map(|member| member.id.clone()).collect();
    let request = json!({ "id": "cli:flock:agents", "method": "agent.list", "params": {} });
    let answers = match crate::flock::client::call_each(&ids, &request) {
        Ok(answers) => answers,
        Err(err) => {
            eprintln!("error: {err}");
            return Ok(1);
        }
    };
    let machines: Vec<Value> = members
        .iter()
        .zip(answers)
        .map(|(member, answer)| {
            let answer = answer.and_then(|response| {
                if let Some(message) = response["error"]["message"].as_str() {
                    return Err(message.to_owned());
                }
                Ok(response["result"]["agents"].clone())
            });
            match answer {
                Ok(agents) => json!({ "id": member.id, "name": member.name, "agents": agents }),
                Err(error) => json!({ "id": member.id, "name": member.name, "error": error }),
            }
        })
        .collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "machines": machines }))?
        );
        return Ok(0);
    }
    println!(
        "{:<14} {:<8} {:<10} {:<9} TITLE",
        "MACHINE", "PANE", "AGENT", "STATUS"
    );
    for machine in &machines {
        let name = machine["name"].as_str().unwrap_or_default();
        if let Some(error) = machine["error"].as_str() {
            println!("{name:<14} {error}");
            continue;
        }
        let agents = machine["agents"].as_array().cloned().unwrap_or_default();
        if agents.is_empty() {
            println!("{name:<14} (no agents)");
        }
        for agent in &agents {
            let field = |key: &str| agent[key].as_str().unwrap_or("-").to_owned();
            println!(
                "{name:<14} {:<8} {:<10} {:<9} {}",
                field("pane_id"),
                field("agent"),
                field("agent_status"),
                field("terminal_title_stripped")
            );
        }
    }
    Ok(0)
}

fn status(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_flag(args, "usage: flockr flock status [--json]") else {
        return Ok(2);
    };
    let dir = store::flock_dir();
    let node = node_running(&dir);
    let hub = store::load_hub(&dir)?;
    let group = store::load_group(&dir)?;
    let (flock, name, me) = match (&hub, &group) {
        (Some(hub), _) => (&hub.group, &hub.name, &hub.hub),
        (None, Some(group)) => (&group.group, &group.name, &group.member),
        (None, None) => {
            eprintln!("{NOT_IN_FLOCK}");
            return Ok(1);
        }
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "group": flock,
                "name": name,
                "member": me,
                "hub": hub.is_some(),
                "hub_addrs": group.as_ref().map(|group| &group.hub_addrs),
                "node": node,
            }))?
        );
        return Ok(0);
    }
    println!("Flock:        {name} ({})", short(flock));
    println!(
        "This machine: {me}, {}",
        if hub.is_some() { "hub" } else { "member" }
    );
    match (&node, &group) {
        (None, _) => println!("Node:         not running"),
        (Some(node), None) => println!(
            "Node:         listening on {}; {} member(s) connected",
            if node.bound.is_empty() {
                "nothing".to_owned()
            } else {
                node.bound.join(", ")
            },
            node.connected.len().saturating_sub(1)
        ),
        (Some(node), Some(group)) => {
            let hub_addrs = group.hub_addrs.join(", ");
            if node.connected.contains(&group.hub) {
                println!("Hub:          connected ({hub_addrs})");
            } else if let Some(reason) = &node.refused {
                println!("Hub:          refused this machine: {reason}");
            } else {
                println!("Hub:          not connected ({hub_addrs})");
            }
        }
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
    let Some(hub) = load_hub(&dir)? else {
        return Ok(1);
    };
    let matches: Vec<&Member> = hub
        .members
        .iter()
        .filter(|member| member.id == *target || member.name == *target)
        .collect();
    let member = match matches.as_slice() {
        [member] => (*member).clone(),
        [] => {
            eprintln!("error: no member {target:?}");
            return Ok(1);
        }
        _ => {
            eprintln!("error: several members are named {target:?}; use the member ID");
            return Ok(1);
        }
    };
    if member.id == hub.hub {
        eprintln!("error: the hub can't remove itself");
        return Ok(1);
    }
    store::update_hub(&dir, |hub| hub.members.retain(|kept| kept.id != member.id))?;
    println!(
        "Removed {} ({}). The hub closes its connection within a few seconds.",
        member.name, member.id
    );
    Ok(0)
}
