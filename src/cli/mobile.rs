//! `flockr mobile ...`: pair phones, list and revoke devices, show gateway status.
//!
//! These commands work on the files in `state_dir()/mobile/` directly and do
//! not need the server to be running.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::mobile::pairing::{self, PairingPayload};
use crate::mobile::store;

const PAIR_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub(super) fn run_mobile_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("pair") => pair(&args[1..]),
        Some("devices") => devices(&args[1..]),
        Some("status") => status(&args[1..]),
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
    eprintln!("flockr mobile commands:");
    eprintln!("  flockr mobile pair [--ttl SECS] [--no-wait] [--json]  show a pairing QR code");
    eprintln!("  flockr mobile devices [list] [--json]                 list paired phones");
    eprintln!("  flockr mobile devices revoke <device_id> | --all      remove paired phones");
    eprintln!("  flockr mobile status [--json]                         show gateway status");
}

fn gateway_listening(dir: &std::path::Path) -> Option<store::GatewayRecord> {
    store::load_gateway(dir).filter(|record| crate::platform::process_exists(record.pid))
}

fn pair(args: &[String]) -> std::io::Result<i32> {
    let mut ttl = None;
    let mut wait = true;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--ttl" => {
                let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    eprintln!("usage: flockr mobile pair [--ttl SECS] [--no-wait] [--json]");
                    return Ok(2);
                };
                ttl = Some(value.clamp(60, 3600));
                index += 1;
            }
            "--no-wait" => wait = false,
            "--json" => {
                json = true;
                wait = false;
            }
            _ => {
                eprintln!("usage: flockr mobile pair [--ttl SECS] [--no-wait] [--json]");
                return Ok(2);
            }
        }
        index += 1;
    }

    let config = crate::config::Config::load().config.mobile;
    let dir = store::mobile_dir();
    let (addrs, _) = config.listen_addrs();
    if !config.enabled || addrs.is_empty() {
        eprintln!(
            "warning: the mobile gateway is off; set [mobile] enabled = true and listen = [\"<ip>\"] in {} and restart the server, or pairing will not complete",
            crate::config::config_path().display()
        );
    } else if gateway_listening(&dir).is_none() {
        eprintln!("warning: the server's mobile gateway is not listening (is the server running and restarted since enabling [mobile]?)");
    }

    let hosts = config.advertised_hosts();
    if hosts.is_empty() {
        eprintln!("error: no host to put in the QR code; set mobile.listen or mobile.advertise");
        return Ok(1);
    }
    let ttl = ttl.unwrap_or_else(|| config.pairing_ttl_secs());
    let sid = store::load_or_create_server_fingerprint(&dir)?;
    let public_key = store::noise_public_key(&store::load_or_create_noise_key(&dir)?)?;
    let now = store::now_unix();
    let (code, record) = pairing::create_pairing(&dir, ttl, now)?;
    let payload = PairingPayload {
        v: pairing::PAYLOAD_VERSION,
        name: config.display_name(),
        hosts,
        port: config.port,
        sid,
        pk: store::base64url(&public_key),
        code,
        exp: record.expires_at,
    };
    let url = payload.to_url();

    if json {
        println!(
            "{}",
            serde_json::json!({ "url": url, "expires_at": record.expires_at })
        );
        return Ok(0);
    }

    match pairing::render_qr(&url) {
        Ok(qr) => println!("{qr}"),
        Err(err) => eprintln!("warning: {err}"),
    }
    println!("Scan with the flockr app, or paste this link into it:");
    println!("{url}");
    println!("Expires in {}:{:02}", ttl / 60, ttl % 60);
    if !wait {
        return Ok(0);
    }

    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        // A second handler cannot be installed; then Ctrl-C just exits and the
        // code dies at its expiry.
        let _ = ctrlc::set_handler(move || interrupted.store(true, Ordering::Release));
    }
    loop {
        if interrupted.load(Ordering::Acquire) {
            remove_if_same(&dir, &record);
            eprintln!("Pairing canceled");
            return Ok(130);
        }
        match store::load_pairing(&dir) {
            Ok(Some(current)) if current.code_sha256 == record.code_sha256 => {
                if let Some(device_id) = current.consumed_by.as_deref() {
                    let name = store::load_devices(&dir)
                        .find(device_id)
                        .map(|device| device.name.clone())
                        .unwrap_or_else(|| "device".to_owned());
                    println!("Paired {name} ({device_id})");
                    let _ = store::remove_pairing(&dir);
                    return Ok(0);
                }
                if !current.is_live(store::now_unix()) {
                    let _ = store::remove_pairing(&dir);
                    println!("Pairing code expired");
                    return Ok(1);
                }
            }
            Ok(_) => {
                eprintln!("Pairing code was replaced by another `flockr mobile pair`");
                return Ok(1);
            }
            Err(err) => {
                eprintln!("error: could not read pairing state: {err}");
                return Ok(1);
            }
        }
        std::thread::sleep(PAIR_POLL_INTERVAL);
    }
}

fn remove_if_same(dir: &std::path::Path, record: &store::PairingRecord) {
    if let Ok(Some(current)) = store::load_pairing(dir) {
        if current.code_sha256 == record.code_sha256 {
            let _ = store::remove_pairing(dir);
        }
    }
}

fn format_unix(secs: u64) -> String {
    if secs == 0 {
        return "-".to_owned();
    }
    time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .ok()
        .map(|date| {
            format!(
                "{}-{:02}-{:02} {:02}:{:02} UTC",
                date.year(),
                u8::from(date.month()),
                date.day(),
                date.hour(),
                date.minute()
            )
        })
        .unwrap_or_else(|| secs.to_string())
}

fn devices(args: &[String]) -> std::io::Result<i32> {
    let dir = store::mobile_dir();
    match args {
        [] => list_devices(&dir, false),
        [flag] if flag == "--json" => list_devices(&dir, true),
        [list] if list == "list" => list_devices(&dir, false),
        [list, flag] if list == "list" && flag == "--json" => list_devices(&dir, true),
        [revoke, all] if revoke == "revoke" && all == "--all" => {
            let (removed, _) =
                store::update_devices(&dir, |devices| std::mem::take(&mut devices.devices))?;
            for device in &removed {
                println!("Revoked {} ({})", device.name, device.device_id);
            }
            if removed.is_empty() {
                println!("No paired devices");
            }
            Ok(0)
        }
        [revoke, id] if revoke == "revoke" && !id.starts_with('-') => {
            if store::load_devices(&dir).find(id).is_none() {
                eprintln!("error: no paired device {id}");
                return Ok(1);
            }
            let (removed, _) = store::update_devices(&dir, |devices| devices.remove(id))?;
            match removed {
                Some(device) => {
                    println!("Revoked {} ({})", device.name, device.device_id);
                    Ok(0)
                }
                None => {
                    eprintln!("error: no paired device {id}");
                    Ok(1)
                }
            }
        }
        _ => {
            eprintln!("usage: flockr mobile devices [list] [--json]");
            eprintln!("       flockr mobile devices revoke <device_id> | --all");
            Ok(2)
        }
    }
}

fn list_devices(dir: &std::path::Path, json: bool) -> std::io::Result<i32> {
    let devices = store::load_devices(dir);
    if json {
        let list = devices
            .devices
            .iter()
            .map(|device| {
                serde_json::json!({
                    "device_id": device.device_id,
                    "name": device.name,
                    "platform": device.platform,
                    "created_at": device.created_at,
                    "last_seen_at": device.last_seen_at,
                    "push": device.push.is_some(),
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::Value::Array(list));
        return Ok(0);
    }
    if devices.devices.is_empty() {
        println!("No paired devices. Pair one with `flockr mobile pair`.");
        return Ok(0);
    }
    println!(
        "{:<20} {:<24} {:<8} {:<22} {:<22} PUSH",
        "ID", "NAME", "PLATFORM", "PAIRED", "LAST SEEN"
    );
    for device in &devices.devices {
        println!(
            "{:<20} {:<24} {:<8} {:<22} {:<22} {}",
            device.device_id,
            device.name.chars().take(24).collect::<String>(),
            device.platform,
            format_unix(device.created_at),
            format_unix(device.last_seen_at),
            if device.push.is_some() { "yes" } else { "no" }
        );
    }
    Ok(0)
}

fn status(args: &[String]) -> std::io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("usage: flockr mobile status [--json]");
            return Ok(2);
        }
    };
    let config = crate::config::Config::load().config.mobile;
    let dir = store::mobile_dir();
    let (addrs, diagnostics) = config.listen_addrs();
    let gateway = gateway_listening(&dir);
    let devices = store::load_devices(&dir);
    let pairing = store::load_pairing(&dir)
        .ok()
        .flatten()
        .filter(|pairing| pairing.is_live(store::now_unix()));
    let key_fingerprint = match store::load_noise_key(&dir) {
        Ok(Some(key)) => store::noise_public_key(&key)
            .map(|public| Some(store::noise_key_fingerprint(&public)))
            .map_err(|err| err.to_string()),
        Ok(None) => Ok(None),
        Err(err) => Err(err.to_string()),
    };
    let public_listen = addrs
        .iter()
        .filter(|addr| crate::mobile::is_public_address(addr.ip()))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "enabled": config.enabled,
                "name": config.display_name(),
                "listen": addrs.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "listen_errors": diagnostics,
                "public_listen": public_listen,
                "key_fingerprint": key_fingerprint.as_ref().ok().cloned().flatten(),
                "bound": gateway.as_ref().map(|record| record.bound.clone()).unwrap_or_default(),
                "running": gateway.is_some(),
                "devices": devices.devices.len(),
                "pairing_expires_at": pairing.as_ref().map(|pairing| pairing.expires_at),
                "push_enabled": config.push.enabled,
            })
        );
        return Ok(0);
    }
    println!("enabled:  {}", config.enabled);
    println!("name:     {}", config.display_name());
    println!(
        "listen:   {}",
        if addrs.is_empty() {
            "-".to_owned()
        } else {
            addrs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    for diagnostic in diagnostics {
        println!("          warning: {diagnostic}");
    }
    for addr in &public_listen {
        println!(
            "          note: {addr} is a public address: {}",
            crate::mobile::PUBLIC_ADDRESS_NOTE
        );
    }
    match &gateway {
        Some(record) if !record.bound.is_empty() => {
            println!("bound:    {}", record.bound.join(", "))
        }
        Some(_) => println!("bound:    none yet (retrying)"),
        None => println!("bound:    not running"),
    }
    match &key_fingerprint {
        Ok(Some(fingerprint)) => println!("key:      {fingerprint}"),
        Ok(None) => println!("key:      none yet (created by `flockr mobile pair` or the server)"),
        Err(err) => println!("key:      error: {err}"),
    }
    println!("devices:  {}", devices.devices.len());
    match pairing {
        Some(pairing) => println!(
            "pairing:  pending, expires {}",
            format_unix(pairing.expires_at)
        ),
        None => println!("pairing:  none"),
    }
    println!(
        "push:     {}",
        if config.push.enabled { "on" } else { "off" }
    );
    Ok(0)
}
