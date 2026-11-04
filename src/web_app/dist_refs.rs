//! Finds the same-origin files a web build's `index.html` loads. `build.rs`
//! includes this file to refuse a partial `app/dist`; the crate compiles it
//! only for its tests. Standard library only, since build scripts share it.

/// Paths, relative to the web build folder, that `<script src="/...">` and
/// `<link href="/...">` tags in `html` load. Query strings and fragments
/// are dropped; protocol-relative (`//host/...`) and bare `/` references are
/// skipped. Each path appears once, in document order.
pub(crate) fn local_asset_paths(html: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let name_end = tag
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(tag.len());
        let name = tag[..name_end].to_ascii_lowercase();
        // Script and style bodies are text, not markup.
        if name == "script" || name == "style" {
            let close = format!("</{name}");
            let body_end = rest.to_ascii_lowercase().find(&close).unwrap_or(rest.len());
            rest = &rest[body_end..];
        }
        let attribute = match name.as_str() {
            "script" => "src",
            "link" => "href",
            _ => continue,
        };
        let Some(value) = attribute_value(&tag[name_end..], attribute) else {
            continue;
        };
        let Some(path) = value.strip_prefix('/') else {
            continue;
        };
        if path.starts_with('/') {
            continue;
        }
        let path = path.split(['?', '#']).next().unwrap_or_default();
        if !path.is_empty() && !paths.iter().any(|seen| seen == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

/// The quoted value of `name` in a tag's attribute text, if present.
fn attribute_value<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if rest.is_empty() {
            return None;
        }
        let key_end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start();
        let Some(after_eq) = rest.strip_prefix('=') else {
            continue;
        };
        rest = after_eq.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = &rest[1..];
                let close = body.find(quote).unwrap_or(body.len());
                rest = body.get(close + 1..).unwrap_or_default();
                &body[..close]
            }
            _ => {
                let close = rest
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(rest.len());
                let value = &rest[..close];
                rest = &rest[close..];
                value
            }
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::local_asset_paths;

    #[test]
    fn finds_scripts_and_links_from_an_expo_export() {
        let html = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <link rel="icon" href="/favicon.ico" sizes="48x48" />
    <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
    <link rel="manifest" href="/site.webmanifest" />
    <style id="expo-reset">body { overflow: hidden; }</style>
  </head>
  <body>
    <div id="root"></div>
  <script src="/_expo/static/js/web/index-50b9.js" defer></script>
</body>
</html>"#;
        assert_eq!(
            local_asset_paths(html),
            [
                "favicon.ico",
                "apple-touch-icon.png",
                "site.webmanifest",
                "_expo/static/js/web/index-50b9.js",
            ]
        );
    }

    #[test]
    fn skips_other_origins_and_non_asset_tags() {
        let html = r#"
<script src="https://cdn.example.com/a.js"></script>
<script src="//cdn.example.com/b.js"></script>
<script>console.log("<link href='/inline'>")</script>
<link rel="preconnect" href="https://fonts.example.com">
<a href="/not-an-asset.html">x</a>
<img src="/not-checked.png">
<link href="/">
<script data-src="/data.js" src="relative.js"></script>
"#;
        assert!(local_asset_paths(html).is_empty());
    }

    #[test]
    fn strips_queries_and_handles_quoting_and_case() {
        let html = "<SCRIPT defer SRC='/app.js?v=1'></SCRIPT>\
                    <link rel=stylesheet href=/style.css#x>\
                    <link href=\"/app.js\">";
        assert_eq!(local_asset_paths(html), ["app.js", "style.css"]);
    }
}
