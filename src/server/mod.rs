mod alt_screen_read;
pub mod autodetect;
pub(crate) mod bind;
#[cfg(unix)]
pub(crate) mod client_accept;
pub(crate) mod client_commands;
mod client_endpoint_control;
pub(crate) mod client_shell;
pub(crate) mod client_shell_graphics;
pub(crate) mod client_transport;
pub(crate) mod clients;
pub(crate) mod clipboard_image;
pub(crate) mod flock;
#[cfg(unix)]
pub(crate) mod handoff;

/// Sent to clients when a live handoff starts. Clients that see it reattach to
/// the replacement server instead of exiting. Part of the wire behavior; keep
/// the text stable. Defined here, outside the Unix-only `handoff` module,
/// because the shared server code refers to it on every platform.
pub(crate) const LIVE_HANDOFF_SHUTDOWN_REASON: &str =
    "live update in progress; reconnect after handoff completes";
pub mod headless;
pub(crate) mod keybindings;
pub(crate) mod mobile;
pub(crate) mod notifications;
pub(crate) mod pane_input;
#[cfg(test)]
mod render_scale_benchmark;
pub(crate) mod render_stream;
pub(crate) mod shutdown;
pub mod socket_paths;
pub(crate) mod terminal_attach;
pub(crate) mod web;
