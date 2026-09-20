use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::types::{CredentialStoreMode, RuntimeInfo};

pub fn codex_home() -> Result<PathBuf, String> {
    if let Ok(value) = env::var("CODEX_HOME") {
        if !value.trim().is_empty() {
            return Ok(PathBuf::from(value));
        }
    }

    #[cfg(windows)]
    let base = env::var_os("USERPROFILE").map(PathBuf::from);

    #[cfg(not(windows))]
    let base = env::var_os("HOME").map(PathBuf::from);

    base.map(|path| path.join(".codex"))
        .ok_or_else(|| "Unable to resolve the user home directory".to_string())
}

pub fn credential_store_mode(codex_home: &Path) -> CredentialStoreMode {
    let config_path = codex_home.join("config.toml");
    let Ok(content) = fs::read_to_string(config_path) else {
        return CredentialStoreMode::File;
    };

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        if key.trim() != "cli_auth_credentials_store" {
            continue;
        }

        let value = value.trim().trim_matches('"');
        return match value {
            "file" => CredentialStoreMode::File,
            "keyring" => CredentialStoreMode::Keyring,
            "auto" => CredentialStoreMode::Auto,
            "ephemeral" => CredentialStoreMode::Ephemeral,
            _ => CredentialStoreMode::Unknown,
        };
    }

    CredentialStoreMode::File
}

pub fn runtime_info() -> Result<RuntimeInfo, String> {
    let home = codex_home()?;
    Ok(RuntimeInfo {
        codex_home: home.display().to_string(),
        auth_file_exists: home.join("auth.json").is_file(),
        credential_store: credential_store_mode(&home),
    })
}

pub fn read_auth_document(codex_home: &Path) -> Result<serde_json::Value, String> {
    let path = codex_home.join("auth.json");
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Unable to read {}: {error}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Invalid Codex auth JSON at {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!("gswitch-{name}-{suffix}"));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn missing_config_defaults_to_file_store() {
        let path = temp_dir("default-store");
        assert_eq!(credential_store_mode(&path), CredentialStoreMode::File);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn reads_explicit_store_mode() {
        let path = temp_dir("explicit-store");
        fs::write(
            path.join("config.toml"),
            "model = \"gpt-5\"\ncli_auth_credentials_store = \"keyring\"\n",
        )
        .expect("write config");

        assert_eq!(credential_store_mode(&path), CredentialStoreMode::Keyring);
        let _ = fs::remove_dir_all(path);
    }
}
