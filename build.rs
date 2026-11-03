use std::io::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/web_app/dist_refs.rs"]
mod dist_refs;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FLOCKR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=FLOCKR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=FLOCKR_BUILD_COMMIT");
    embed_web_app();
}

/// Packs the built web app into `$OUT_DIR/web-app.zlib` for `src/web_app.rs`.
///
/// The folder is `FLOCKR_WEB_DIST` when set, else `app/dist`
/// (`npm run export:web`). Without an `index.html` there the file is empty
/// and the binary has no web app, so builds work without Node.
///
/// Files and folders whose names start with `.` are left out, and symlinks
/// are skipped with a warning; that matches what the release workflow's
/// artifact upload ships. If `index.html` loads a same-origin script or
/// link that is not in the folder, the build is partial: with
/// `FLOCKR_WEB_DIST` set (release builds) that fails the build, otherwise
/// it warns and embeds nothing.
///
/// Format before zlib: for each file, sorted by path, a u32 LE path length,
/// the `/`-separated relative path, a u64 LE data length, and the data.
///
/// A missing folder is not watched: Cargo reruns build scripts on every
/// build while a watched path is missing. After the first web export, touch
/// build.rs (or set FLOCKR_WEB_DIST) so the next build picks it up.
fn embed_web_app() {
    println!("cargo:rerun-if-env-changed=FLOCKR_WEB_DIST");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("web-app.zlib");
    let required = std::env::var_os("FLOCKR_WEB_DIST").filter(|dir| !dir.is_empty());
    let dist = match &required {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("app/dist"),
    };
    if !dist.join("index.html").is_file() {
        if required.is_some() {
            panic!(
                "FLOCKR_WEB_DIST={} has no index.html; build the web app first",
                dist.display()
            );
        }
        std::fs::write(&out, []).expect("write empty web app archive");
        return;
    }
    println!("cargo:rerun-if-changed={}", dist.display());

    let mut files = Vec::new();
    collect_files(&dist, &dist, &mut files);
    files.sort();

    let index = std::fs::read_to_string(dist.join("index.html"))
        .unwrap_or_else(|err| panic!("read {}: {err}", dist.join("index.html").display()));
    let missing: Vec<String> = dist_refs::local_asset_paths(&index)
        .into_iter()
        .filter(|asset| !files.iter().any(|(name, _)| name == asset))
        .collect();
    if !missing.is_empty() {
        let list = missing.join(", ");
        if required.is_some() {
            panic!(
                "{} is a partial web build; index.html loads files that are not there: {list}",
                dist.display()
            );
        }
        println!(
            "cargo:warning=not embedding the web app: {} is a partial build; index.html loads missing files: {list}. Run npm run export:web in app/ again.",
            dist.display()
        );
        std::fs::write(&out, []).expect("write empty web app archive");
        return;
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    for (name, path) in &files {
        let data =
            std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let name_len = u32::try_from(name.len()).expect("web app path fits in u32");
        let data_len = u64::try_from(data.len()).expect("web app file fits in u64");
        encoder
            .write_all(&name_len.to_le_bytes())
            .and_then(|()| encoder.write_all(name.as_bytes()))
            .and_then(|()| encoder.write_all(&data_len.to_le_bytes()))
            .and_then(|()| encoder.write_all(&data))
            .expect("compress web app");
    }
    let archive = encoder.finish().expect("compress web app");
    std::fs::write(&out, archive).expect("write web app archive");
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display()));
    for entry in entries {
        let entry = entry.expect("read web app folder entry");
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // DirEntry::file_type does not follow symlinks.
        let file_type = entry
            .file_type()
            .unwrap_or_else(|err| panic!("stat {}: {err}", path.display()));
        if file_type.is_symlink() {
            println!("cargo:warning=web app: skipping symlink {}", path.display());
        } else if file_type.is_dir() {
            collect_files(root, &path, files);
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("file is under the web app folder");
            let name = relative
                .components()
                .map(|part| part.as_os_str().to_str().expect("web app paths are UTF-8"))
                .collect::<Vec<_>>()
                .join("/");
            files.push((name, path));
        }
    }
}
