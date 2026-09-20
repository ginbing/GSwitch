use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{json, Value};
use uuid::Uuid;

pub struct AppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl AppServer {
    pub fn start(codex_home: &Path) -> Result<Self, String> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .env("CODEX_HOME", codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Unable to start Codex App Server: {error}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex App Server stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex App Server stdout is unavailable".to_string())?;

        let mut server = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };

        server.send(json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": "gswitch",
                    "title": "GSwitch",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))?;
        server.read_response(0)?;
        server.send(json!({"method": "initialized", "params": {}}))?;

        Ok(server)
    }

    pub fn send(&mut self, message: Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, &message)
            .map_err(|error| format!("Unable to encode Codex App Server request: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|error| format!("Unable to write Codex App Server request: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("Unable to flush Codex App Server request: {error}"))
    }

    pub fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| format!("Unable to read Codex App Server response: {error}"))?;
        if bytes == 0 {
            return Err("Codex App Server exited unexpectedly".to_string());
        }

        serde_json::from_str(&line)
            .map_err(|error| format!("Invalid Codex App Server response: {error}"))
    }

    pub fn read_response(&mut self, id: i64) -> Result<Value, String> {
        loop {
            let message = self.read_message()?;
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }

            if let Some(error) = message.get("error") {
                return Err(format!("Codex App Server request failed: {error}"));
            }

            return message
                .get("result")
                .cloned()
                .ok_or_else(|| "Codex App Server response is missing result".to_string());
        }
    }

    pub fn account_read(&mut self, id: i64, refresh_token: bool) -> Result<Value, String> {
        self.send(json!({
            "method": "account/read",
            "id": id,
            "params": { "refreshToken": refresh_token }
        }))?;
        self.read_response(id)
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct TempCodexHome {
    pub path: PathBuf,
}

impl TempCodexHome {
    pub fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("gswitch-{}", Uuid::new_v4()));
        fs::create_dir(&path)
            .map_err(|error| format!("Unable to create temporary Codex profile: {error}"))?;

        let configure = || -> Result<(), String> {
            set_private_permissions(&path, 0o700)?;
            let config_path = path.join("config.toml");
            fs::write(&config_path, "cli_auth_credentials_store = \"file\"\n")
                .map_err(|error| format!("Unable to configure temporary Codex profile: {error}"))?;
            set_private_permissions(&config_path, 0o600)
        };

        if let Err(error) = configure() {
            let _ = fs::remove_dir_all(&path);
            return Err(error);
        }

        Ok(Self { path })
    }

    pub fn write_auth(&self, credential: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(credential)
            .map_err(|error| format!("Unable to serialize imported credentials: {error}"))?;
        let path = self.path.join("auth.json");
        fs::write(&path, bytes)
            .map_err(|error| format!("Unable to write temporary auth file: {error}"))?;
        set_private_permissions(&path, 0o600)
    }

    pub fn read_auth(&self) -> Result<Value, String> {
        let path = self.path.join("auth.json");
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Unable to read {}: {error}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|error| format!("Invalid Codex auth JSON at {}: {error}", path.display()))
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("Unable to inspect temporary credential path: {error}"))?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("Unable to protect temporary credential path: {error}"))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

impl Drop for TempCodexHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct AccountMetadata {
    pub kind: crate::types::AccountKind,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

pub fn account_metadata(result: &Value) -> Result<AccountMetadata, String> {
    let account = result
        .get("account")
        .filter(|value| !value.is_null())
        .ok_or_else(|| "Codex did not recognize an account".to_string())?;

    let account_type = account
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex account response is missing type".to_string())?;

    let kind = match account_type {
        "chatgpt" => crate::types::AccountKind::ChatGpt,
        "apiKey" | "apikey" => crate::types::AccountKind::ApiKey,
        other => return Err(format!("Unsupported Codex account type: {other}")),
    };

    Ok(AccountMetadata {
        kind,
        email: account
            .get("email")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        plan_type: account
            .get("planType")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chatgpt_account_metadata() {
        let value = json!({
            "account": {
                "type": "chatgpt",
                "email": "user@example.com",
                "planType": "pro"
            },
            "requiresOpenaiAuth": true
        });
        let metadata = account_metadata(&value).expect("metadata");
        assert_eq!(metadata.kind, crate::types::AccountKind::ChatGpt);
        assert_eq!(metadata.email.as_deref(), Some("user@example.com"));
        assert_eq!(metadata.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn rejects_missing_account() {
        let value = json!({"account": null, "requiresOpenaiAuth": true});
        assert!(account_metadata(&value).is_err());
    }

    #[test]
    fn temporary_profile_round_trips_complete_credential_document() {
        let profile = TempCodexHome::create().expect("profile");
        let credential = json!({
            "tokens": {"access_token": "secret", "refresh_token": "rotate-me"},
            "auth_mode": "chatgpt",
            "future_field": {"preserve": true}
        });

        profile.write_auth(&credential).expect("write auth");
        assert_eq!(profile.read_auth().expect("read auth"), credential);
    }
}
