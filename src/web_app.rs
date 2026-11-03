//! The web app built into the binary. `build.rs` packs `app/dist` (or
//! `FLOCKR_WEB_DIST`) into a zlib archive when a web build is present;
//! without one the archive is empty and this binary has no web app.

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, OnceLock};

// Shared with build.rs, which uses it to refuse a partial web build.
#[cfg(test)]
mod dist_refs;

static ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/web-app.zlib"));

/// Web app files by `/`-separated relative path (`index.html`,
/// `_expo/static/js/web/index-<hash>.js`).
pub(crate) type WebFiles = HashMap<String, Vec<u8>>;

/// Whether this binary was built with a web app.
pub(crate) fn is_embedded() -> bool {
    !ARCHIVE.is_empty()
}

/// The embedded files, decompressed on first use and kept in memory.
/// `None` when the binary has no web app or its archive is damaged.
pub(crate) fn files() -> Option<Arc<WebFiles>> {
    static FILES: OnceLock<Option<Arc<WebFiles>>> = OnceLock::new();
    FILES
        .get_or_init(|| {
            if !is_embedded() {
                return None;
            }
            match decode(ARCHIVE) {
                Ok(files) => Some(Arc::new(files)),
                Err(err) => {
                    tracing::warn!(err = %err, "built-in web app is damaged");
                    None
                }
            }
        })
        .clone()
}

/// Reads the archive format written by `build.rs`.
fn decode(archive: &[u8]) -> std::io::Result<WebFiles> {
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(archive).read_to_end(&mut raw)?;
    let mut files = WebFiles::new();
    let mut rest = raw.as_slice();
    while !rest.is_empty() {
        let name_len = take_len::<4>(&mut rest)?;
        let name = String::from_utf8(take(&mut rest, name_len)?.to_vec())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let data_len = take_len::<8>(&mut rest)?;
        let data = take(&mut rest, data_len)?.to_vec();
        files.insert(name, data);
    }
    if !files.contains_key("index.html") {
        return Err(invalid("no index.html"));
    }
    Ok(files)
}

fn take_len<const N: usize>(rest: &mut &[u8]) -> std::io::Result<usize> {
    let mut bytes = [0_u8; 8];
    bytes[..N].copy_from_slice(take(rest, N)?);
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| invalid("length too large"))
}

fn take<'a>(rest: &mut &'a [u8], len: usize) -> std::io::Result<&'a [u8]> {
    if rest.len() < len {
        return Err(invalid("truncated"));
    }
    let (head, tail) = rest.split_at(len);
    *rest = tail;
    Ok(head)
}

fn invalid(reason: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write as _;

    /// Builds an archive the way `build.rs` does.
    pub(crate) fn encode(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        for (name, data) in files {
            encoder
                .write_all(&(name.len() as u32).to_le_bytes())
                .unwrap();
            encoder.write_all(name.as_bytes()).unwrap();
            encoder
                .write_all(&(data.len() as u64).to_le_bytes())
                .unwrap();
            encoder.write_all(data).unwrap();
        }
        encoder.finish().unwrap()
    }

    #[test]
    fn decodes_the_build_script_format() {
        let archive = encode(&[
            ("index.html", b"<html>"),
            ("_expo/static/js/web/app.js", b"js"),
            ("empty.txt", b""),
        ]);
        let files = decode(&archive).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files["index.html"], b"<html>");
        assert_eq!(files["_expo/static/js/web/app.js"], b"js");
        assert_eq!(files["empty.txt"], b"");
    }

    #[test]
    fn rejects_damaged_archives() {
        assert!(decode(b"not zlib").is_err());
        assert!(decode(&encode(&[("app.js", b"js")])).is_err());
        let archive = encode(&[("index.html", b"<html>")]);
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(archive.as_slice())
            .read_to_end(&mut raw)
            .unwrap();
        raw.truncate(raw.len() - 1);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        assert!(decode(&encoder.finish().unwrap()).is_err());
    }

    /// Runs both ways: with `app/dist` built the binary must carry a working
    /// web app; without it, none.
    #[test]
    fn embedded_archive_matches_the_build() {
        let dist = std::env::var_os("FLOCKR_WEB_DIST")
            .filter(|dir| !dir.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app/dist"));
        if !is_embedded() {
            assert!(files().is_none());
            return;
        }
        let files = files().expect("embedded web app decodes");
        assert!(files.contains_key("index.html"));
        if dist.join("index.html").is_file() {
            assert_eq!(
                files["index.html"],
                std::fs::read(dist.join("index.html")).unwrap()
            );
        }
    }
}
