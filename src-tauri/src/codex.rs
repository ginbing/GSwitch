use std::{
    env, fs,
    io::ErrorKind,
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

pub fn credential_store_mode(codex_home: &Path) -> Result<CredentialStoreMode, String> {
    let config_path = codex_home.join("config.toml");
    let content = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(CredentialStoreMode::File),
        Err(_) => return Err("Unable to inspect Codex configuration".to_string()),
    };

    let config: toml_edit::DocumentMut = content
        .parse()
        .map_err(|_| "Unable to parse Codex configuration".to_string())?;
    let value = config
        .get("cli_auth_credentials_store")
        .and_then(|item| item.as_str());

    Ok(match value {
        None | Some("file") => CredentialStoreMode::File,
        Some("keyring") => CredentialStoreMode::Keyring,
        Some("auto") => CredentialStoreMode::Auto,
        Some("ephemeral") => CredentialStoreMode::Ephemeral,
        Some(_) => CredentialStoreMode::Unknown,
    })
}

pub fn runtime_info() -> Result<RuntimeInfo, String> {
    let home = codex_home()?;
    let auth_file_exists = match fs::metadata(home.join("auth.json")) {
        Ok(metadata) => metadata.is_file(),
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => return Err("Unable to inspect Codex credentials".to_string()),
    };

    Ok(RuntimeInfo {
        codex_home: home.display().to_string(),
        auth_file_exists,
        credential_store: credential_store_mode(&home)?,
    })
}

pub fn auth_path(codex_home: &Path) -> PathBuf {
    codex_home.join("auth.json")
}

pub fn read_auth_document(codex_home: &Path) -> Result<serde_json::Value, String> {
    crate::storage::read_json(&auth_path(codex_home), "Codex credentials")
}

/// Distinguishes a missing file (a normal signed-out state) from an unreadable
/// or malformed credential document. Callers must not overwrite the latter.
pub fn read_optional_auth_document(codex_home: &Path) -> Result<Option<serde_json::Value>, String> {
    let path = auth_path(codex_home);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content)
            .map(Some)
            .map_err(|_| "Unable to parse Codex credentials".to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Unable to read Codex credentials".to_string()),
    }
}

pub fn write_auth_document(
    codex_home: &Path,
    credential: &serde_json::Value,
) -> Result<(), String> {
    crate::storage::write_json_atomic(&auth_path(codex_home), credential, "Codex credentials")
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
    fn missing_config_uses_the_official_file_default() {
        let path = temp_dir("default-store");
        assert_eq!(
            credential_store_mode(&path).expect("store mode"),
            CredentialStoreMode::File
        );
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn parses_toml_without_being_confused_by_comments() {
        let path = temp_dir("explicit-store");
        fs::write(
            path.join("config.toml"),
            "model = \"gpt-5\"\n# cli_auth_credentials_store = \"auto\"\ncli_auth_credentials_store = \"keyring\"\n",
        )
        .expect("write config");

        assert_eq!(
            credential_store_mode(&path).expect("store mode"),
            CredentialStoreMode::Keyring
        );
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn malformed_config_is_not_mistaken_for_file_mode() {
        let path = temp_dir("invalid-store");
        fs::write(path.join("config.toml"), "cli_auth_credentials_store = [")
            .expect("write config");

        assert_eq!(
            credential_store_mode(&path).expect_err("invalid config"),
            "Unable to parse Codex configuration"
        );
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn distinguishes_signed_out_from_invalid_credentials() {
        let path = temp_dir("optional-auth");
        assert_eq!(
            read_optional_auth_document(&path).expect("signed out"),
            None
        );

        fs::write(path.join("auth.json"), "{").expect("write invalid auth");
        assert_eq!(
            read_optional_auth_document(&path).expect_err("invalid auth"),
            "Unable to parse Codex credentials"
        );
        let _ = fs::remove_dir_all(path);
    }
}
