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
            .map_err(|error| format!("Unable to encode App Server request: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|error| format!("Unable to write App Server request: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("Unable to flush App Server request: {error}"))
    }

    pub fn read_message(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| format!("Unable to read Codex App Server: {error}"))?;
        if read == 0 {
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
                .ok_or_else(|| "Codex App Server response has no result".to_string());
        }
    }

    pub fn account_read(&mut self, id: i64) -> Result<Value, String> {
        self.send(json!({
            "method": "account/read",
            "id": id,
            "params": { "refreshToken": false }
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
        fs::create_dir_all(&path)
            .map_err(|error| format!("Unable to create temporary Codex profile: {error}"))?;
        fs::write(
            path.join("config.toml"),
            "cli_auth_credentials_store = \"file\"\n",
        )
        .map_err(|error| format!("Unable to configure temporary Codex profile: {error}"))?;
        Ok(Self { path })
    }

    pub fn write_auth(&self, credential: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(credential)
            .map_err(|error| format!("Unable to serialize imported credentials: {error}"))?;
        fs::write(self.path.join("auth.json"), bytes)
            .map_err(|error| format!("Unable to write temporary auth file: {error}"))
    }

    pub fn read_auth(&self) -> Result<Value, String> {
        let path = self.path.join("auth.json");
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("Unable to read {}: {error}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|error| format!("Invalid Codex auth JSON at {}: {error}", path.display()))
    }
}

impl Drop for TempCodexHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn account_metadata(result: &Value) -> Result<(String, Option<String>, Option<String>), String> {
    let account = result
        .get("account")
        .filter(|value| !value.is_null())
        .ok_or_else(|| "Codex did not recognize an account".to_string())?;

    let account_type = account
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let email = account
        .get("email")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let plan_type = account
        .get("planType")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok((account_type, email, plan_type))
}
