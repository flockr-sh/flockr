//! Lets plugins written for upstream run unchanged: every `FLOCKR_*` variable
//! a plugin gets also comes as its upstream name, and the upstream command
//! name runs this binary.

const UPSTREAM_ENV_PREFIX: &str = "HERDR_";

// Only Unix links the command; Windows still names it in tests.
#[cfg_attr(not(unix), allow(dead_code))]
const UPSTREAM_COMMAND: &str = "herdr";

/// The upstream name for a `FLOCKR_*` variable.
pub(crate) fn upstream_env_key(key: &str) -> Option<String> {
    key.strip_prefix("FLOCKR_")
        .map(|rest| format!("{UPSTREAM_ENV_PREFIX}{rest}"))
}

/// The `FLOCKR_*` name for an upstream variable.
pub(crate) fn flockr_env_key(key: &str) -> Option<String> {
    key.strip_prefix(UPSTREAM_ENV_PREFIX)
        .map(|rest| format!("FLOCKR_{rest}"))
}

/// Adds the upstream twin of every `FLOCKR_*` entry, and puts a directory
/// whose upstream command runs this binary first on `PATH`. Without the
/// command, a plugin calling it would reach a real upstream install, which
/// would then find this server's socket in its environment.
pub(super) fn add_upstream_env(env: &mut Vec<(String, String)>) {
    let twins = env
        .iter()
        .filter_map(|(key, value)| upstream_env_key(key).map(|key| (key, value.clone())))
        .collect::<Vec<_>>();
    env.extend(twins);
    if let Some(dir) = upstream_command_dir() {
        let path = env
            .iter()
            .rev()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        let separator = if cfg!(windows) { ";" } else { ":" };
        env.retain(|(key, _)| key != "PATH");
        env.push((
            "PATH".to_string(),
            format!("{}{separator}{path}", dir.display()),
        ));
    }
}

#[cfg(unix)]
fn upstream_command_dir() -> Option<std::path::PathBuf> {
    let target = crate::platform::launch_executable().ok()?;
    let dir = crate::config::state_dir().join("plugin-bin");
    let link = dir.join(UPSTREAM_COMMAND);
    if std::fs::read_link(&link).ok().as_deref() != Some(target.as_path()) {
        std::fs::create_dir_all(&dir).ok()?;
        let _ = std::fs::remove_file(&link);
        if let Err(err) = std::os::unix::fs::symlink(&target, &link) {
            tracing::warn!(%err, path = %link.display(), "could not link the upstream plugin command");
            return None;
        }
    }
    Some(dir)
}

// Windows has no cheap symlink for an executable; plugins there get the
// environment twins only.
#[cfg(not(unix))]
fn upstream_command_dir() -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_env_gets_upstream_twins_and_command_dir() {
        let mut env = vec![
            ("FLOCKR_PLUGIN_ID".to_string(), "demo".to_string()),
            (
                "FLOCKR_SOCKET_PATH".to_string(),
                "/tmp/flockr.sock".to_string(),
            ),
            ("OTHER".to_string(), "x".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ];
        add_upstream_env(&mut env);
        let get = |key: &str| {
            env.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let upstream = |rest: &str| format!("{UPSTREAM_ENV_PREFIX}{rest}");
        assert_eq!(get(&upstream("PLUGIN_ID")), Some("demo"));
        assert_eq!(get(&upstream("SOCKET_PATH")), Some("/tmp/flockr.sock"));
        assert_eq!(get(&upstream("OTHER")), None);
        assert_eq!(env.iter().filter(|(key, _)| key == "PATH").count(), 1);
        #[cfg(unix)]
        {
            let path = get("PATH").unwrap();
            let (dir, rest) = path.split_once(':').unwrap();
            assert_eq!(rest, "/usr/bin");
            let link = std::path::Path::new(dir).join(UPSTREAM_COMMAND);
            assert_eq!(
                std::fs::read_link(link).ok(),
                crate::platform::launch_executable().ok()
            );
        }
    }

    #[test]
    fn upstream_manifest_name_and_version_key_load() {
        let root = std::env::temp_dir().join(format!(
            "flockr-upstream-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(format!("{UPSTREAM_COMMAND}-plugin.toml")),
            format!(
                "id = 'example.upstream'\nname = 'Upstream'\nversion = '0.1.0'\nmin_{UPSTREAM_COMMAND}_version = '0.6.10'\n"
            ),
        )
        .unwrap();
        let plugin = crate::app::load_plugin_manifest(&root.display().to_string(), true);
        std::fs::remove_dir_all(&root).unwrap();
        let plugin = plugin.expect("upstream manifest loads");
        assert_eq!(plugin.plugin_id, "example.upstream");
        assert_eq!(plugin.min_flockr_version, "0.6.10");
    }

    #[test]
    fn env_keys_map_both_ways() {
        let upstream = format!("{UPSTREAM_ENV_PREFIX}BIN_PATH");
        assert_eq!(upstream_env_key("FLOCKR_BIN_PATH"), Some(upstream.clone()));
        assert_eq!(
            flockr_env_key(&upstream).as_deref(),
            Some("FLOCKR_BIN_PATH")
        );
        assert_eq!(upstream_env_key("PATH"), None);
    }
}
