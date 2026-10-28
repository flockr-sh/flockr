use clap::{Arg, ArgAction, Command};

use super::{json_flag, option};

fn text() -> Arg {
    Arg::new("text").value_name("TEXT").num_args(1..)
}

fn wait_args(command: Command) -> Command {
    command
        .arg(
            Arg::new("wait")
                .long("wait")
                .action(ArgAction::SetTrue)
                .help("Wait for the reply and print it"),
        )
        .arg(option("timeout", "MS").help("How long --wait waits (default 600000)"))
        .arg(json_flag())
}

pub(super) fn command() -> Command {
    Command::new("message")
        .about("Send messages to agents on any machine in the flock")
        .subcommand(wait_args(
            Command::new("send")
                .about("Send a message to an agent (machine:agent or agent)")
                .arg(Arg::new("to").value_name("TO"))
                .arg(text()),
        ))
        .subcommand(wait_args(
            Command::new("reply")
                .about("Reply to a message you received")
                .arg(Arg::new("id").value_name("MESSAGE_ID"))
                .arg(text()),
        ))
        .subcommand(
            Command::new("read")
                .about("Read this agent's unread messages")
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("log")
                .about("Show this machine's message log")
                .arg(option("limit", "N").help("Events to show (default 50)"))
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("pending")
                .about("List messages waiting for approval")
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("approve")
                .about("Approve a waiting message")
                .arg(Arg::new("id").value_name("MESSAGE_ID"))
                .arg(
                    Arg::new("always")
                        .long("always")
                        .action(ArgAction::SetTrue)
                        .help("Allow this sender to message this agent from now on"),
                ),
        )
        .subcommand(
            Command::new("deny")
                .about("Deny a waiting message")
                .arg(Arg::new("id").value_name("MESSAGE_ID")),
        )
        .subcommand(
            Command::new("cancel")
                .about("Cancel a conversation on this machine")
                .arg(Arg::new("conversation").value_name("CONVERSATION_ID")),
        )
}
