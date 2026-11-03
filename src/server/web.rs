//! `[web]`: serves the flockr web app's files over plain HTTP, like other
//! local web UIs. The app itself talks to the `[mobile]` gateway; this only
//! hands out its static files. GET and HEAD only; unknown paths get
//! `index.html` so the app's own routes load. The files come from `[web]
//! dir` when set, else from the web app built into the binary.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::WebConfig;
use crate::web_app::WebFiles;

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Keeps the web server running; dropping it stops it.
pub(crate) struct WebHandle {
    shutdown: watch::Sender<bool>,
}

impl Drop for WebHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Where the web app's files come from.
#[derive(Clone)]
enum Source {
    /// `[web] dir`.
    Dir(PathBuf),
    /// The web app built into the binary.
    Embedded(Arc<WebFiles>),
}

impl Source {
    /// `dir` when set, else the built-in app. `Err` says why there is
    /// nothing to serve.
    fn pick(dir: Option<&str>, embedded: Option<Arc<WebFiles>>) -> Result<Self, String> {
        match dir {
            Some(dir) if Path::new(dir).join("index.html").is_file() => {
                Ok(Self::Dir(PathBuf::from(dir)))
            }
            Some(dir) => Err(format!(
                "web.dir {dir:?} has no index.html; not serving the web app"
            )),
            None => embedded.map(Self::Embedded).ok_or_else(|| {
                "web.enabled is true but web.dir is not set and this flockr has no built-in \
                 web app; not serving the web app"
                    .to_owned()
            }),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Dir(dir) => dir.display().to_string(),
            Self::Embedded(_) => "built-in".to_owned(),
        }
    }

    /// The body and content type for a request target.
    async fn lookup(&self, target: &str) -> Option<(Cow<'_, [u8]>, &'static str)> {
        let parts = request_parts(target)?;
        match self {
            Self::Dir(dir) => {
                let path = resolve_parts(dir, &parts);
                let body = tokio::fs::read(&path).await.ok()?;
                Some((Cow::Owned(body), content_type(&path)))
            }
            Self::Embedded(files) => {
                let (name, body) = embedded_file(files, &parts)?;
                Some((Cow::Borrowed(body), content_type(Path::new(name))))
            }
        }
    }
}

/// Starts serving when `[web]` enables it. Must run inside the tokio runtime.
pub(crate) fn start(config: &WebConfig) -> Option<WebHandle> {
    if !config.enabled {
        return None;
    }
    let (addrs, diagnostics) = config.listen_addrs();
    for diagnostic in diagnostics {
        warn!("{diagnostic}");
    }
    if addrs.is_empty() {
        warn!("web.enabled is true but web.listen has no usable address");
        return None;
    }
    let embedded = if config.dir().is_none() {
        crate::web_app::files()
    } else {
        None
    };
    let source = match Source::pick(config.dir(), embedded) {
        Ok(source) => source,
        Err(reason) => {
            warn!("{reason}");
            return None;
        }
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    for addr in addrs {
        tokio::spawn(listen(addr, source.clone(), shutdown_rx.clone()));
    }
    Some(WebHandle { shutdown })
}

async fn listen(addr: SocketAddr, source: Source, mut shutdown: watch::Receiver<bool>) {
    let Some(listener) = crate::server::bind::bind_with_retry(addr, "web app", &mut shutdown).await
    else {
        return;
    };
    info!(%addr, files = %source.describe(), "web app listening");
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let source = source.clone();
                    tokio::spawn(async move {
                        let served = tokio::time::timeout(REQUEST_TIMEOUT, serve(stream, &source)).await;
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

async fn serve(mut stream: TcpStream, source: &Source) -> std::io::Result<()> {
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
    match source.lookup(target).await {
        Some((body, content_type)) => respond(&mut stream, 200, content_type, &body, head).await,
        None => respond(&mut stream, 404, "text/plain", b"not found", head).await,
    }
}

/// The path segments of a request target. Targets that would leave the web
/// app's folder get `None`.
fn request_parts(target: &str) -> Option<Vec<String>> {
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_decode(path)?;
    let mut parts = Vec::new();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts)
}

/// The file for a request target inside `dir`. Paths that leave `dir` get
/// `None`; anything that isn't a file gets `index.html`.
#[cfg(test)]
fn resolve(dir: &Path, target: &str) -> Option<PathBuf> {
    request_parts(target).map(|parts| resolve_parts(dir, &parts))
}

fn resolve_parts(dir: &Path, parts: &[String]) -> PathBuf {
    let file = parts
        .iter()
        .fold(dir.to_path_buf(), |file, part| file.join(part));
    if file.is_file() {
        file
    } else {
        dir.join("index.html")
    }
}

/// The built-in file for request path segments; anything that isn't a file
/// gets `index.html`.
fn embedded_file<'a>(files: &'a WebFiles, parts: &[String]) -> Option<(&'a str, &'a [u8])> {
    files
        .get_key_value(&parts.join("/"))
        .or_else(|| files.get_key_value("index.html"))
        .map(|(name, body)| (name.as_str(), body.as_slice()))
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
        "webmanifest" => "application/manifest+json",
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
    use tokio::net::TcpListener;

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
        assert_eq!(
            content_type(Path::new("site.webmanifest")),
            "application/manifest+json"
        );
    }

    #[test]
    fn embedded_paths_stay_inside_and_fall_back_to_index() {
        let files: WebFiles = [
            ("index.html", "<html>"),
            ("_expo/static/js/web/app.js", "js"),
            ("canvaskit.wasm", "wasm"),
        ]
        .into_iter()
        .map(|(name, body)| (name.to_owned(), body.as_bytes().to_vec()))
        .collect();
        let lookup = |target: &str| {
            request_parts(target).and_then(|parts| {
                embedded_file(&files, &parts).map(|(name, body)| (name.to_owned(), body.to_vec()))
            })
        };
        assert_eq!(
            lookup("/_expo/static/js/web/app.js?v=1"),
            Some(("_expo/static/js/web/app.js".to_owned(), b"js".to_vec()))
        );
        assert_eq!(
            lookup("/%5Fexpo/static/js/web/app.js"),
            Some(("_expo/static/js/web/app.js".to_owned(), b"js".to_vec()))
        );
        assert_eq!(
            lookup("/./canvaskit.wasm").map(|(name, _)| name),
            Some("canvaskit.wasm".to_owned())
        );
        assert_eq!(
            lookup("/").map(|(name, _)| name),
            Some("index.html".to_owned())
        );
        assert_eq!(
            lookup("/_expo").map(|(name, _)| name),
            Some("index.html".to_owned())
        );
        assert_eq!(
            lookup("/servers/x").map(|(name, _)| name),
            Some("index.html".to_owned())
        );
        assert_eq!(lookup("/../canvaskit.wasm"), None);
        assert_eq!(lookup("/%2e%2e/canvaskit.wasm"), None);
    }

    #[test]
    fn dir_overrides_the_built_in_app_and_missing_builds_serve_nothing() {
        let dir = crate::mobile::store::tests::temp_dir("web-pick");
        std::fs::write(dir.join("index.html"), "<html>").unwrap();
        let dir_text = dir.to_str().unwrap();
        let embedded = Arc::new(WebFiles::from([(
            "index.html".to_owned(),
            b"<html>".to_vec(),
        )]));

        assert!(matches!(
            Source::pick(Some(dir_text), Some(embedded.clone())),
            Ok(Source::Dir(picked)) if picked == dir
        ));
        assert!(matches!(
            Source::pick(None, Some(embedded.clone())),
            Ok(Source::Embedded(_))
        ));
        let missing = dir.join("missing");
        let error = Source::pick(missing.to_str(), Some(embedded))
            .err()
            .unwrap();
        assert!(error.contains("has no index.html"), "{error}");
        let error = Source::pick(None, None).err().unwrap();
        assert!(error.contains("no built-in web app"), "{error}");
    }

    #[test]
    fn start_does_not_listen_without_an_app_or_address() {
        let dir = crate::mobile::store::tests::temp_dir("web-start");
        let config = WebConfig {
            enabled: true,
            listen: vec!["127.0.0.1:1".to_owned()],
            dir: Some(dir.join("missing").display().to_string()),
            ..WebConfig::default()
        };
        assert!(start(&config).is_none());
        let config = WebConfig {
            enabled: true,
            dir: Some(dir.display().to_string()),
            ..WebConfig::default()
        };
        assert!(start(&config).is_none());
    }

    async fn get(source: Source, target: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, &source).await.unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn serves_files_over_http() {
        let dir = crate::mobile::store::tests::temp_dir("web-serve");
        std::fs::write(dir.join("index.html"), "<html>hi</html>").unwrap();
        let response = get(Source::Dir(dir), "/").await;
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("text/html"));
        assert!(response.ends_with("<html>hi</html>"));
    }

    #[tokio::test]
    async fn serves_built_in_files_over_http() {
        let files = WebFiles::from([
            ("index.html".to_owned(), b"<html>hi</html>".to_vec()),
            ("canvaskit.wasm".to_owned(), b"wasm".to_vec()),
        ]);
        let source = Source::Embedded(Arc::new(files));
        let response = get(source.clone(), "/canvaskit.wasm").await;
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(
            response.contains("Content-Type: application/wasm"),
            "{response}"
        );
        assert!(response.ends_with("wasm"));
        let response = get(source, "/servers/x").await;
        assert!(response.contains("text/html"), "{response}");
        assert!(response.ends_with("<html>hi</html>"));
    }
}
