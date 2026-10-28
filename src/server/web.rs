//! `[web]`: serves the flockr web app's files over plain HTTP, like other
//! local web UIs. The app itself talks to the `[mobile]` gateway; this only
//! hands out its static files. GET and HEAD only; unknown paths get
//! `index.html` so the app's own routes load.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::WebConfig;

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const BIND_RETRY: Duration = Duration::from_secs(10);

/// Keeps the web server running; dropping it stops it.
pub(crate) struct WebHandle {
    shutdown: watch::Sender<bool>,
}

impl Drop for WebHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Starts serving when `[web]` enables it. Must run inside the tokio runtime.
pub(crate) fn start(config: &WebConfig) -> Option<WebHandle> {
    if !config.enabled {
        return None;
    }
    for diagnostic in config.diagnostics() {
        warn!("{diagnostic}");
    }
    let dir = PathBuf::from(config.dir.as_deref()?.trim());
    let (addrs, _) = config.listen_addrs();
    if addrs.is_empty() {
        return None;
    }
    let (shutdown, shutdown_rx) = watch::channel(false);
    for addr in addrs {
        tokio::spawn(listen(addr, dir.clone(), shutdown_rx.clone()));
    }
    Some(WebHandle { shutdown })
}

async fn listen(addr: SocketAddr, dir: PathBuf, mut shutdown: watch::Receiver<bool>) {
    let listener = loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => break listener,
            Err(err) => {
                warn!(%addr, err = %err, "web app could not bind; retrying in 10 s");
                tokio::select! {
                    _ = tokio::time::sleep(BIND_RETRY) => {}
                    _ = shutdown.changed() => return,
                }
            }
        }
    };
    info!(%addr, dir = %dir.display(), "web app listening");
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let dir = dir.clone();
                    tokio::spawn(async move {
                        let served = tokio::time::timeout(REQUEST_TIMEOUT, serve(stream, &dir)).await;
                        if !matches!(served, Ok(Ok(()))) {
                            debug!(%peer, "web request failed or timed out");
                        }
                    });
                }
                Err(err) => {
                    warn!(err = %err, "web accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => return,
        }
    }
}

async fn serve(mut stream: TcpStream, dir: &Path) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..read]);
        if request.len() > MAX_REQUEST_BYTES {
            return respond(&mut stream, 431, "text/plain", b"request too large", true).await;
        }
    }
    let line = String::from_utf8_lossy(&request);
    let mut parts = line.lines().next().unwrap_or_default().split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or("/"),
    );
    let head = method == "HEAD";
    if method != "GET" && !head {
        return respond(&mut stream, 405, "text/plain", b"method not allowed", head).await;
    }
    let Some(path) = resolve(dir, target) else {
        return respond(&mut stream, 404, "text/plain", b"not found", head).await;
    };
    match tokio::fs::read(&path).await {
        Ok(body) => respond(&mut stream, 200, content_type(&path), &body, head).await,
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found", head).await,
    }
}

/// The file for a request target inside `dir`. Paths that leave `dir` get
/// `None`; anything that isn't a file gets `index.html`.
fn resolve(dir: &Path, target: &str) -> Option<PathBuf> {
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(path)?;
    let mut file = dir.to_path_buf();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => file.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if file.is_file() {
        Some(file)
    } else {
        Some(dir.join("index.html"))
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head: bool,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Request Header Fields Too Large",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    if !head {
        stream.write_all(body).await?;
    }
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_folder_and_fall_back_to_index() {
        let dir = crate::mobile::store::tests::temp_dir("web-resolve");
        std::fs::write(dir.join("index.html"), "<html>").unwrap();
        std::fs::create_dir_all(dir.join("_expo")).unwrap();
        std::fs::write(dir.join("_expo/app.js"), "js").unwrap();
        assert_eq!(
            resolve(&dir, "/_expo/app.js?v=1"),
            Some(dir.join("_expo/app.js"))
        );
        assert_eq!(
            resolve(&dir, "/%5Fexpo/app.js"),
            Some(dir.join("_expo/app.js"))
        );
        assert_eq!(resolve(&dir, "/"), Some(dir.join("index.html")));
        assert_eq!(resolve(&dir, "/servers/x"), Some(dir.join("index.html")));
        assert_eq!(resolve(&dir, "/../etc/passwd"), None);
        assert_eq!(resolve(&dir, "/%2e%2e/etc/passwd"), None);
        assert_eq!(content_type(Path::new("a.wasm")), "application/wasm");
    }

    #[tokio::test]
    async fn serves_files_over_http() {
        let dir = crate::mobile::store::tests::temp_dir("web-serve");
        std::fs::write(dir.join("index.html"), "<html>hi</html>").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let served_dir = dir.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, &served_dir).await.unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("text/html"));
        assert!(response.ends_with("<html>hi</html>"));
    }
}
