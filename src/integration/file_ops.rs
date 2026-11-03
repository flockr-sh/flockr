use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::FLOCKR_SKILL;

pub(crate) fn remove_file_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(windows)]
pub(crate) fn legacy_bash_hook_path(hook_path: &Path) -> std::path::PathBuf {
    hook_path.with_file_name("flockr-agent-state.sh")
}

#[cfg(windows)]
pub(crate) fn remove_legacy_bash_hook_file(hook_path: &Path) -> io::Result<bool> {
    let legacy_path = legacy_bash_hook_path(hook_path);
    let content = match fs::read_to_string(&legacy_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    if content.contains("FLOCKR_INTEGRATION_ID=") {
        fs::remove_file(legacy_path)?;
        return Ok(true);
    }

    Ok(false)
}

#[cfg(not(windows))]
pub(crate) fn remove_legacy_bash_hook_file(_hook_path: &Path) -> io::Result<bool> {
    Ok(false)
}

pub(crate) fn remove_dir_all_if_exists(path: &Path) -> io::Result<bool> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn make_executable(_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(_path, perms)?;
    }

    Ok(())
}

/// Writes the flockr skill to `<agent dir>/skills/flockr/SKILL.md`.
pub(crate) fn install_skill(agent_dir: &Path) -> io::Result<PathBuf> {
    let skill_dir = agent_dir.join("skills").join("flockr");
    fs::create_dir_all(&skill_dir)?;
    let path = skill_dir.join("SKILL.md");
    fs::write(&path, FLOCKR_SKILL)?;
    Ok(path)
}

/// The path `install_skill` writes for this agent directory.
pub(crate) fn skill_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("skills").join("flockr").join("SKILL.md")
}

/// Removes the flockr skill, and its folder when nothing else is in it.
pub(crate) fn uninstall_skill(agent_dir: &Path) -> io::Result<bool> {
    let path = skill_path(agent_dir);
    let removed = remove_file_if_exists(&path)?;
    if let Some(dir) = path.parent() {
        // Fails when the folder still holds other files; leave those.
        let _ = fs::remove_dir(dir);
    }
    Ok(removed)
}
