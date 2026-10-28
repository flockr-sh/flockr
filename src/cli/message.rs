//! `flockr message ...`: send messages to agents on any machine in the flock,
//! read your own, and approve, deny or cancel them.
//!
//! These commands talk to this machine's flock node on `flock.sock`. An agent
//! runs them from its own pane; `FLOCKR_PANE_ID` tells the node who it is.

use std::io::{BufRead as _, BufReader, Write as _};

use serde_json::{json, Value};

use crate::flock::message;

pub(super) fn run_message_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("send") => send(&args[1..], None),
        Some("reply") => match args.get(1) {
            Some(id) => send(&args[2..], Some(id)),
            None => usage("flockr message reply <message-id> <text> [--wait] [--timeout MS]"),
        },
        Some("read") => read(&args[1..]),
        Some("log") => log(&args[1..]),
        Some("pending") => pending(&args[1..]),
        Some("approve") => approve(&args[1..]),
        Some("deny") => simple(
            &args[1..],
            "message.deny",
            "id",
            "flockr message deny <message-id>",
        ),
        Some("cancel") => simple(
            &args[1..],
            "message.cancel",
            "conversation",
            "flockr message cancel <conversation-id>",
        ),
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
    eprintln!("flockr message commands:");
    eprintln!("  flockr message send <machine:agent|agent> <text> [--wait] [--timeout MS]");
    eprintln!("  flockr message reply <message-id> <text> [--wait] [--timeout MS]");
    eprintln!("  flockr message read [--json]            read this agent's unread messages");
    eprintln!("  flockr message log [--limit N] [--json] show this machine's message log");
    eprintln!("  flockr message pending [--json]         messages waiting for approval");
    eprintln!("  flockr message approve <message-id> [--always]");
    eprintln!("  flockr message deny <message-id>");
    eprintln!("  flockr message cancel <conversation-id>");
    eprintln!();
    eprintln!("Agents are named by agent name or pane ID; prefix the machine's flock name");
    eprintln!("for another machine (tower:reviewer). Agents in the same git repo may message");
    eprintln!("each other; anything else waits for a person to approve it.");
}

fn usage(text: &str) -> std::io::Result<i32> {
    eprintln!("usage: {text}");
    Ok(2)
}

/// Sends one request to the flock node and returns its result, or prints
/// the error and returns `None`.
fn call(method: &str, mut params: Value) -> std::io::Result<Option<Value>> {
    if let Ok(pane) = std::env::var("FLOCKR_PANE_ID") {
        if !pane.trim().is_empty() {
            params["caller_pane"] = json!(pane);
        }
    }
    let path = message::socket_path();
    let mut stream = match crate::ipc::connect_local_stream(&path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("error: this machine's flock node isn't running ({err}); enable [flock] and start the server");
            return Ok(None);
        }
    };
    let request = json!({ "id": format!("cli:{method}"), "method": method, "params": params });
    let mut line = request.to_string().into_bytes();
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    let answer: Value = serde_json::from_str(&answer)
        .map_err(|err| std::io::Error::other(format!("bad answer from the flock node: {err}")))?;
    if let Some(error) = answer.get("error") {
        eprintln!(
            "error: {}",
            error["message"]
                .as_str()
                .unwrap_or("the flock node refused")
        );
        return Ok(None);
    }
    Ok(Some(answer["result"].clone()))
}

fn print_json(value: &Value) -> std::io::Result<i32> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(0)
}

fn print_message(message: &Value) {
    let from = &message["from"];
    let sender = match from["agent"].as_str() {
        Some(agent) => format!(
            "{}:{agent} (another agent, not your user)",
            from["machine"].as_str().unwrap_or("?")
        ),
        None => format!("{} (a person)", from["machine"].as_str().unwrap_or("?")),
    };
    let id = message["id"].as_str().unwrap_or("?");
    println!("Message {id} from {sender}");
    println!(
        "Conversation {}. Reply with: flockr message reply {id} \"<text>\"",
        message["conversation"].as_str().unwrap_or("?")
    );
    println!("<<<");
    println!("{}", message["body"].as_str().unwrap_or_default());
    println!(">>>");
}

fn send(args: &[String], reply_to: Option<&String>) -> std::io::Result<i32> {
    let usage_text = if reply_to.is_some() {
        "flockr message reply <message-id> <text> [--wait] [--timeout MS]"
    } else {
        "flockr message send <machine:agent|agent> <text> [--wait] [--timeout MS]"
    };
    let mut positional = Vec::new();
    let mut wait = false;
    let mut timeout = None;
    let mut as_json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--wait" => wait = true,
            "--json" => as_json = true,
            "--timeout" => {
                let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    return usage(usage_text);
                };
                timeout = Some(value);
                index += 1;
            }
            _ => positional.push(args[index].clone()),
        }
        index += 1;
    }
    let (to, body) = match (reply_to, positional.as_slice()) {
        (Some(_), body) if !body.is_empty() => (None, body.join(" ")),
        (None, [to, body @ ..]) if !body.is_empty() => (Some(to.clone()), body.join(" ")),
        _ => return usage(usage_text),
    };
    let mut params = json!({ "body": body, "wait": wait });
    if let Some(to) = to {
        params["to"] = json!(to);
    }
    if let Some(reply_to) = reply_to {
        params["reply_to"] = json!(reply_to);
    }
    if let Some(timeout) = timeout {
        params["timeout_ms"] = json!(timeout);
    }
    let Some(result) = call("message.send", params)? else {
        return Ok(1);
    };
    if as_json {
        return print_json(&result);
    }
    let status = match result["status"].as_str() {
        Some("pending_approval") => "waiting for a person on that machine to approve it",
        Some("queued") => "queued; the agent is told when it is idle",
        Some(other) => other,
        None => "sent",
    };
    println!(
        "Sent {} to {}: {status}.",
        result["id"].as_str().unwrap_or("?"),
        result["to"].as_str().unwrap_or("?")
    );
    if result["reply"].is_object() {
        println!();
        print_message(&result["reply"]);
    }
    Ok(0)
}

fn json_only(args: &[String], usage_text: &str) -> Option<bool> {
    match args {
        [] => Some(false),
        [flag] if flag == "--json" => Some(true),
        _ => {
            eprintln!("usage: {usage_text}");
            None
        }
    }
}

fn read(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_only(args, "flockr message read [--json]") else {
        return Ok(2);
    };
    let Some(result) = call("message.read", json!({}))? else {
        return Ok(1);
    };
    if as_json {
        return print_json(&result);
    }
    let messages = result["messages"].as_array().cloned().unwrap_or_default();
    if messages.is_empty() {
        println!("No unread messages.");
    }
    for (index, message) in messages.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_message(message);
    }
    Ok(0)
}

fn log(args: &[String]) -> std::io::Result<i32> {
    let mut limit = 50;
    let mut as_json = false;
    let mut index = 0;
    while index < args.len() {
        match (args[index].as_str(), args.get(index + 1)) {
            ("--json", _) => as_json = true,
            ("--limit", Some(value)) if value.parse::<u64>().is_ok() => {
                limit = value.parse().unwrap_or(limit);
                index += 1;
            }
            _ => return usage("flockr message log [--limit N] [--json]"),
        }
        index += 1;
    }
    let Some(result) = call("message.log", json!({ "limit": limit }))? else {
        return Ok(1);
    };
    if as_json {
        return print_json(&result);
    }
    for event in result["events"].as_array().into_iter().flatten() {
        let text = |key: &str| event[key].as_str().unwrap_or("-").to_owned();
        let detail = event["detail"]
            .as_str()
            .map(|detail| format!(" ({detail})"))
            .unwrap_or_default();
        println!(
            "{} {:<9} {} -> {}{detail}: {}",
            event["at"].as_u64().unwrap_or_default(),
            text("event"),
            text("from"),
            text("to"),
            text("body").replace('\n', " ")
        );
    }
    Ok(0)
}

fn pending(args: &[String]) -> std::io::Result<i32> {
    let Some(as_json) = json_only(args, "flockr message pending [--json]") else {
        return Ok(2);
    };
    let Some(result) = call("message.pending", json!({}))? else {
        return Ok(1);
    };
    if as_json {
        return print_json(&result);
    }
    let pending = result["pending"].as_array().cloned().unwrap_or_default();
    if pending.is_empty() {
        println!("No messages are waiting for approval.");
    }
    for message in &pending {
        println!(
            "{}  {} -> {}",
            message["id"].as_str().unwrap_or("?"),
            party_label(&message["from"]),
            party_label(&message["to"])
        );
        println!(
            "  {}",
            message["body"]
                .as_str()
                .unwrap_or_default()
                .replace('\n', "\n  ")
        );
    }
    Ok(0)
}

fn party_label(party: &Value) -> String {
    let machine = party["machine"].as_str().unwrap_or("?");
    match party["agent"].as_str() {
        Some(agent) => format!("{machine}:{agent}"),
        None => format!("{machine} (a person)"),
    }
}

fn approve(args: &[String]) -> std::io::Result<i32> {
    let (id, always) = match args {
        [id] => (id, false),
        [id, flag] | [flag, id] if flag == "--always" => (id, true),
        _ => return usage("flockr message approve <message-id> [--always]"),
    };
    let Some(result) = call("message.approve", json!({ "id": id, "always": always }))? else {
        return Ok(1);
    };
    println!(
        "Approved {} to {}{}.",
        result["id"].as_str().unwrap_or("?"),
        result["to"].as_str().unwrap_or("?"),
        if always {
            "; this sender may message this agent from now on"
        } else {
            ""
        }
    );
    Ok(0)
}

fn simple(args: &[String], method: &str, key: &str, usage_text: &str) -> std::io::Result<i32> {
    let [value] = args else {
        return usage(usage_text);
    };
    let Some(result) = call(method, json!({ key: value }))? else {
        return Ok(1);
    };
    print_json(&result)
}
