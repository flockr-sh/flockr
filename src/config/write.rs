#[derive(Clone, Copy)]
pub(crate) enum ConfigEdit<'a> {
    /// Write `key = value` in `[table]`. `value` is a TOML literal.
    Set {
        table: &'a str,
        key: &'a str,
        value: &'a str,
    },
    /// Remove the key from `[table]` so its default applies.
    Reset { table: &'a str, key: &'a str },
}

impl ConfigEdit<'_> {
    pub(crate) fn description(self) -> &'static str {
        "setting"
    }

    pub(crate) fn apply(self, content: &str) -> String {
        match self {
            Self::Set { table, key, value } => {
                let content = super::upsert_section_value(content, table, key, value);
                // The legacy ui.toast.enabled key would override delivery.
                if table == "ui.toast" && key == "delivery" {
                    super::remove_section_key(&content, "ui.toast", "enabled")
                } else {
                    content
                }
            }
            Self::Reset { table, key } => super::remove_section_key(content, table, key),
        }
    }
}

pub(crate) fn update_file_at(
    path: &std::path::Path,
    description: &str,
    update: impl FnOnce(&str) -> String,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config directory: {error}"))?;
    }
    let content = match super::io::read_optional_config(path) {
        Ok(Some(content)) => content,
        Ok(None) => String::new(),
        Err(error) => {
            return Err(format!(
                "failed to read config before saving {description}: {error}"
            ));
        }
    };
    std::fs::write(path, update(&content))
        .map_err(|error| format!("failed to save {description}: {error}"))
}

pub(crate) fn write_edit(edit: ConfigEdit<'_>) -> Result<(), String> {
    update_file_at(&super::config_path(), edit.description(), |content| {
        edit.apply(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_file_at_does_not_move_a_leading_bom_into_the_file() {
        let dir = std::env::temp_dir().join(format!("flockr-config-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            b"\xEF\xBB\xBF[terminal]\ndefault_shell = \"pwsh.exe\"\n",
        )
        .unwrap();

        update_file_at(&path, "onboarding setting", |content| {
            crate::config::upsert_top_level_bool(content, "onboarding", false)
        })
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert!(
            !written.contains('\u{feff}'),
            "unexpected BOM in {written:?}"
        );
        assert!(
            toml::from_str::<toml::Value>(&written).is_ok(),
            "written config is not valid TOML: {written:?}"
        );
    }
}
