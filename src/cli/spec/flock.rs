use clap::{Arg, Command};

use super::{json_flag, option};

pub(super) fn command() -> Command {
    Command::new("flock")
        .about("Join several machines' flockr servers through one hub")
        .subcommand(
            Command::new("init")
                .about("Start a flock with this machine as the hub")
                .arg(option("name", "NAME").help("Flock name")),
        )
        .subcommand(
            Command::new("invite")
                .about("Invite a machine and approve its join (on the hub)")
                .arg(option("role", "ROLE").help("admin (default), member, or viewer"))
                .arg(option("ttl", "SECS").help("Invite lifetime in seconds (60..=3600)")),
        )
        .subcommand(
            Command::new("join")
                .about("Join a flock with an invite URL")
                .arg(Arg::new("url").value_name("URL")),
        )
        .subcommand(
            Command::new("members")
                .about("List members")
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("status")
                .about("Show this machine's flock state")
                .arg(json_flag()),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove a member (on the hub)")
                .arg(Arg::new("member").value_name("MEMBER")),
        )
}
