//! Clickable desktop notifications through the freedesktop notification D-Bus API.

use std::{
    collections::HashMap,
    io,
    process::{Command, Stdio},
    sync::{Arc, OnceLock},
};

use futures_util::StreamExt;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

type Callback = Arc<dyn Fn() + Send + Sync>;

const DEFAULT_ACTION: &str = "default";

struct Request {
    title: String,
    body: Option<String>,
    key: String,
    callback: Callback,
}

static SERVICE: OnceLock<UnboundedSender<Request>> = OnceLock::new();

/// Show a notification that runs `callback` when the user clicks it. A later
/// notification with the same `key` replaces the earlier one.
pub(crate) fn show_actionable_desktop_notification(
    title: &str,
    body: Option<&str>,
    key: String,
    callback: Callback,
) -> io::Result<bool> {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Ok(false);
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return super::show_desktop_notification(title, body);
    };
    let service = SERVICE.get_or_init(|| {
        let (sender, receiver) = mpsc::unbounded_channel();
        runtime.spawn(run(receiver));
        sender
    });
    service
        .send(Request {
            title: title.to_owned(),
            body: body.filter(|body| !body.is_empty()).map(str::to_owned),
            key,
            callback,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "notification service stopped"))?;
    Ok(true)
}

async fn run(mut requests: UnboundedReceiver<Request>) {
    while let Some(request) = requests.recv().await {
        let connection = match zbus::Connection::session().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::debug!(%error, "session bus unavailable; notification is not clickable");
                show_plain(request);
                continue;
            }
        };
        match serve(&connection, request, &mut requests).await {
            Ok(()) => return,
            Err(error) => tracing::debug!(%error, "notification service lost its connection"),
        }
    }
}

async fn serve(
    connection: &zbus::Connection,
    first: Request,
    requests: &mut UnboundedReceiver<Request>,
) -> zbus::Result<()> {
    let notifications = zbus::Proxy::new(
        connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await?;
    // Subscribe before the first Notify so an early click is not missed.
    let mut invoked = notifications.receive_signal("ActionInvoked").await?;
    let mut closed = notifications.receive_signal("NotificationClosed").await?;
    let mut shown = Shown::default();
    notify(&notifications, &mut shown, first).await;
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else { return Ok(()) };
                notify(&notifications, &mut shown, request).await;
            }
            signal = invoked.next() => {
                let Some(signal) = signal else { return Err(zbus::Error::InputOutput(lost())) };
                let (id, action): (u32, String) = signal.body().deserialize()?;
                if action == DEFAULT_ACTION {
                    if let Some(callback) = shown.callback(id) {
                        callback();
                    }
                }
            }
            signal = closed.next() => {
                let Some(signal) = signal else { return Err(zbus::Error::InputOutput(lost())) };
                let (id, _reason): (u32, u32) = signal.body().deserialize()?;
                shown.remove(id);
            }
        }
    }
}

fn lost() -> Arc<io::Error> {
    Arc::new(io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "notification signal stream ended",
    ))
}

async fn notify(notifications: &zbus::Proxy<'_>, shown: &mut Shown, request: Request) {
    let replaces_id = shown.id(&request.key).unwrap_or(0);
    let hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    let result: zbus::Result<u32> = notifications
        .call(
            "Notify",
            &(
                "Flockr",
                replaces_id,
                "",
                request.title.as_str(),
                request.body.as_deref().unwrap_or(""),
                vec![DEFAULT_ACTION, "Open"],
                hints,
                -1i32,
            ),
        )
        .await;
    match result {
        Ok(id) => shown.insert(request.key, id, request.callback),
        Err(error) => {
            tracing::debug!(%error, "notification daemon rejected Notify; showing a plain one");
            show_plain(request);
        }
    }
}

fn show_plain(request: Request) {
    tokio::task::spawn_blocking(move || {
        if let Err(error) =
            super::show_desktop_notification(&request.title, request.body.as_deref())
        {
            tracing::warn!(%error, "failed to emit system notification");
        }
    });
}

/// Notifications this client has open, by replacement key and daemon id.
#[derive(Default)]
struct Shown {
    ids: HashMap<String, u32>,
    callbacks: HashMap<u32, Callback>,
}

impl Shown {
    fn id(&self, key: &str) -> Option<u32> {
        self.ids.get(key).copied()
    }

    fn insert(&mut self, key: String, id: u32, callback: Callback) {
        if let Some(previous) = self.ids.insert(key, id) {
            if previous != id {
                self.callbacks.remove(&previous);
            }
        }
        self.callbacks.insert(id, callback);
    }

    fn callback(&self, id: u32) -> Option<Callback> {
        self.callbacks.get(&id).cloned()
    }

    fn remove(&mut self, id: u32) {
        if self.callbacks.remove(&id).is_some() {
            self.ids.retain(|_, shown| *shown != id);
        }
    }
}

/// Raise the terminal window that hosts this client.
///
/// Only Hyprland is supported: the client walks its parent processes to the
/// first one that owns a Hyprland window and focuses that window.
pub(crate) fn foreground_desktop_notification_host() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("flockr-focus-host".into())
        .spawn(|| {
            if let Err(error) = focus_hyprland_host_window() {
                tracing::debug!(%error, "could not focus the terminal window");
            }
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "could not start terminal focus thread");
    }
}

#[derive(serde::Deserialize)]
struct HyprlandClient {
    pid: i64,
    address: String,
    #[serde(rename = "focusHistoryID", default)]
    focus_history_id: i64,
}

fn focus_hyprland_host_window() -> io::Result<()> {
    let output = Command::new("hyprctl")
        .args(["clients", "-j"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("hyprctl clients failed"));
    }
    let clients: Vec<HyprlandClient> = serde_json::from_slice(&output.stdout)?;
    let address = host_window_address(&clients, std::process::id(), |pid| {
        super::process_name_and_parent(pid).map(|(_, parent)| parent)
    })
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no ancestor owns a window"))?;
    let window = format!("address:{address}");
    // Hyprland 0.56 takes Lua dispatchers; older releases take `focuswindow`.
    let lua = format!("hl.dsp.focus({{ window = \"{window}\" }})");
    if hyprctl_dispatch(&[lua.as_str()])? || hyprctl_dispatch(&["focuswindow", &window])? {
        return Ok(());
    }
    Err(io::Error::other("hyprctl could not focus the window"))
}

/// hyprctl exits 0 for some failed dispatches; only an `ok` reply is success.
fn hyprctl_dispatch(args: &[&str]) -> io::Result<bool> {
    let output = Command::new("hyprctl")
        .arg("dispatch")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    Ok(output.status.success() && output.stdout.trim_ascii() == b"ok")
}

/// Find the window of the nearest ancestor process that has one. A terminal
/// that serves several windows from one process gets its most recently focused
/// window.
fn host_window_address(
    clients: &[HyprlandClient],
    pid: u32,
    parent: impl Fn(u32) -> Option<u32>,
) -> Option<&str> {
    let mut pid = pid;
    // Bound the walk in case /proc reports a cycle.
    for _ in 0..64 {
        if let Some(client) = clients
            .iter()
            .filter(|client| client.pid == i64::from(pid))
            .min_by_key(|client| client.focus_history_id)
        {
            return Some(&client.address);
        }
        pid = parent(pid).filter(|parent| *parent > 1)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::time::Duration;

    fn client(pid: i64, address: &str, focus_history_id: i64) -> HyprlandClient {
        HyprlandClient {
            pid,
            address: address.into(),
            focus_history_id,
        }
    }

    #[test]
    fn host_window_is_the_nearest_ancestor_window() {
        let clients = [client(10, "0xterm", 1), client(30, "0xother", 0)];
        let parents = HashMap::from([(30, 20), (20, 10), (10, 1)]);
        let parent = |pid| parents.get(&pid).copied();
        assert_eq!(host_window_address(&clients, 30, parent), Some("0xother"));
        assert_eq!(host_window_address(&clients, 20, parent), Some("0xterm"));
        assert_eq!(host_window_address(&[], 30, parent), None);
    }

    #[test]
    fn host_window_prefers_the_most_recently_focused_window_of_a_process() {
        let clients = [client(10, "0xold", 3), client(10, "0xrecent", 1)];
        assert_eq!(
            host_window_address(&clients, 10, |_| None),
            Some("0xrecent")
        );
    }

    #[test]
    fn closing_a_notification_forgets_its_callback_and_key() {
        let mut shown = Shown::default();
        let callback: Callback = Arc::new(|| {});
        shown.insert("pane".into(), 4, callback.clone());
        shown.insert("pane".into(), 5, callback);
        assert!(shown.callback(4).is_none(), "replaced id is dropped");
        assert_eq!(shown.id("pane"), Some(5));
        shown.remove(5);
        assert!(shown.callback(5).is_none());
        assert_eq!(shown.id("pane"), None);
    }

    struct PrivateBus(std::process::Child);

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct Daemon {
        calls: mpsc::UnboundedSender<(u32, Vec<String>)>,
        next_id: u32,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Daemon {
        #[allow(clippy::too_many_arguments)] // The D-Bus method signature is fixed by the spec.
        fn notify(
            &mut self,
            _app_name: String,
            replaces_id: u32,
            _app_icon: String,
            _summary: String,
            _body: String,
            actions: Vec<String>,
            _hints: HashMap<String, zbus::zvariant::OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            let id = if replaces_id == 0 {
                self.next_id += 1;
                self.next_id
            } else {
                replaces_id
            };
            let _ = self.calls.send((replaces_id, actions));
            id
        }
    }

    #[tokio::test]
    #[ignore = "requires dbus-daemon; uses a private bus"]
    async fn clicking_the_default_action_runs_the_callback_and_replacement_reuses_the_id() {
        let mut bus = PrivateBus(
            std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let (calls, mut call_rx) = mpsc::unbounded_channel();
        let daemon = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at(
                "/org/freedesktop/Notifications",
                Daemon { calls, next_id: 0 },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let client = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .build()
            .await
            .unwrap();
        let (clicked, mut clicked_rx) = mpsc::unbounded_channel();
        let request = |title: &str| {
            let clicked = clicked.clone();
            Request {
                title: title.into(),
                body: None,
                key: "pane".into(),
                callback: Arc::new(move || {
                    let _ = clicked.send(());
                }),
            }
        };
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let first = request("first");
        let service = tokio::spawn(async move { serve(&client, first, &mut receiver).await });
        let wait = Duration::from_secs(3);
        let (replaces, actions) = tokio::time::timeout(wait, call_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replaces, 0);
        assert_eq!(actions, ["default", "Open"]);

        sender.send(request("second")).unwrap();
        let (replaces, _) = tokio::time::timeout(wait, call_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replaces, 1, "same key replaces the open notification");

        let emit = |member: &'static str, body: (u32, String)| {
            let daemon = daemon.clone();
            async move {
                daemon
                    .emit_signal(
                        None::<&str>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        member,
                        &body,
                    )
                    .await
                    .unwrap();
            }
        };
        emit("ActionInvoked", (99, DEFAULT_ACTION.into())).await;
        emit("ActionInvoked", (1, "other".into())).await;
        emit("ActionInvoked", (1, DEFAULT_ACTION.into())).await;
        tokio::time::timeout(wait, clicked_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            clicked_rx.try_recv().is_err(),
            "only the default action on our id clicks"
        );

        drop(sender);
        tokio::time::timeout(wait, service)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
