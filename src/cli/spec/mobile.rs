use clap::{Arg, Command};

use super::{flag, json_flag, option};

pub(super) fn command() -> Command {
    Command::new("mobile")
        .about("Pair phones with the flockr mobile app and manage them")
        .subcommand(
            Command::new("pair")
                .about("Show a pairing QR code and wait for a phone to scan it")
                .arg(option("ttl", "SECS").help("Code lifetime in seconds (60..=3600)"))
                .arg(flag("no-wait").help("Print the code and exit without waiting"))
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("devices")
                .about("List or revoke paired phones")
                .arg(json_flag())
                .subcommand(
                    Command::new("list")
                        .about("List paired phones")
                        .arg(json_flag()),
                )
                .subcommand(
                    Command::new("revoke")
                        .about("Remove a paired phone; its connections close within 2 seconds")
                        .arg(Arg::new("device_id").value_name("DEVICE_ID"))
                        .arg(flag("all").help("Remove every paired phone")),
                ),
        )
        .subcommand(
            Command::new("status")
                .about("Show the mobile gateway's addresses, devices, and pending pairing")
                .arg(json_flag()),
        )
}
