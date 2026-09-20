use std::{
    collections::VecDeque,
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::storage;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AppServer {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value, String>>,
    pending_messages: VecDeque<Value>,
}

impl AppServer {
    pub fn start(codex_home: &Path) -> Result<Self, String> {
        let mut command = app_server_command(codex_home);
        let mut child = command.spawn().map_err(|_| {
            "Unable to start Codex App Server. Check that Codex is installed.".to_string()
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex App Server did not provide standard input".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex App Server did not provide standard output".to_string())?;
        let messages = spawn_reader(stdout, &mut child)?;

        let mut server = Self {
            child,
            stdin,
            messages,
            pending_messages: VecDeque::new(),
        };

        server.call(
            0,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "gswitch",
                    "title": "GSwitch",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            STARTUP_TIMEOUT,
        )?;
        server.send(json!({"method": "initialized", "params": {}}))?;

        Ok(server)
    }

    pub fn call(
        &mut self,
        id: i64,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.send(json!({"method": method, "id": id, "params": params}))?;
        self.read_response_with_timeout(id, timeout)
    }

    pub fn send(&mut self, message: Value) -> Result<(), String> {
        ensure_child_is_running(&mut self.child)?;
        serde_json::to_writer(&mut self.stdin, &message)
            .map_err(|_| "Unable to encode a Codex App Server request".to_string())?;
        self.stdin
            .write_all(b"\n")
            .map_err(|_| "Unable to write to Codex App Server".to_string())?;
        self.stdin
            .flush()
            .map_err(|_| "Unable to flush a Codex App Server request".to_string())
    }

    pub fn read_response_with_timeout(
        &mut self,
        id: i64,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.take_pending(|message| response_id(message) == Some(id)) {
                return response_result(message);
            }

            let message = self.next_message(remaining(deadline)?)?;
            if response_id(&message) == Some(id) {
                return response_result(message);
            }
            self.pending_messages.push_back(message);
        }
    }

    pub fn wait_for_notification<F>(
        &mut self,
        timeout: Duration,
        mut matches: F,
    ) -> Result<Value, String>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.take_pending(|message| matches(message)) {
                return Ok(message);
            }

            let message = self.next_message(remaining(deadline)?)?;
            if matches(&message) {
                return Ok(message);
            }
            self.pending_messages.push_back(message);
        }
    }

    pub fn account_read(&mut self, id: i64, refresh_token: bool) -> Result<Value, String> {
        self.call(
            id,
            "account/read",
            json!({"refreshToken": refresh_token}),
            REQUEST_TIMEOUT,
        )
    }

    fn take_pending<F>(&mut self, matches: F) -> Option<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let index = self.pending_messages.iter().position(matches)?;
        self.pending_messages.remove(index)
    }

    fn next_message(&mut self, timeout: Duration) -> Result<Value, String> {
        receive_message(&self.messages, timeout)
    }
}

fn receive_message(
    messages: &Receiver<Result<Value, String>>,
    timeout: Duration,
) -> Result<Value, String> {
    match messages.recv_timeout(timeout) {
        Ok(message) => message,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("Codex App Server did not respond before the operation timed out".to_string())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Codex App Server exited unexpectedly".to_string())
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn app_server_command(codex_home: &Path) -> Command {
    #[cfg(windows)]
    let mut command = {
        let invocation = configured_codex_binary()
            .map(|path| format!("\"\"{}\" app-server\"", path.display()))
            .unwrap_or_else(|| "codex app-server".to_string());
        let mut command = Command::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
        command.args(["/D", "/S", "/C", &invocation]);
        command
    };

    #[cfg(not(windows))]
    let mut command =
        Command::new(configured_codex_binary().unwrap_or_else(|| PathBuf::from("codex")));

    command
        .current_dir(codex_home)
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for name in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
        "OPENAI_ORG_ID",
        "OPENAI_ORGANIZATION",
        "OPENAI_PROJECT",
    ] {
        command.env_remove(name);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
}

fn configured_codex_binary() -> Option<PathBuf> {
    let explicit = env::var_os("GSWITCH_CODEX_BIN")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    if explicit.is_some() {
        return explicit;
    }

    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("npm").join("codex.cmd"))
            .filter(|path| path.is_file())
    }

    #[cfg(not(windows))]
    {
        None
    }
}

fn spawn_reader(
    stdout: ChildStdout,
    child: &mut Child,
) -> Result<Receiver<Result<Value, String>>, String> {
    let (sender, receiver) = mpsc::channel();
    let reader = thread::Builder::new()
        .name("gswitch-app-server-reader".to_string())
        .spawn(move || read_messages(stdout, sender));

    if reader.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Unable to monitor Codex App Server responses".to_string());
    }
    Ok(receiver)
}

fn read_messages(stdout: ChildStdout, sender: mpsc::Sender<Result<Value, String>>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let message = serde_json::from_str(&line)
                    .map_err(|_| "Codex App Server returned an invalid message".to_string());
                if sender.send(message).is_err() {
                    break;
                }
            }
            Err(_) => {
                let _ = sender.send(Err("Unable to read from Codex App Server".to_string()));
                break;
            }
        }
    }
}

fn ensure_child_is_running(child: &mut Child) -> Result<(), String> {
    match child
        .try_wait()
        .map_err(|_| "Unable to inspect Codex App Server".to_string())?
    {
        Some(_) => Err("Codex App Server exited unexpectedly".to_string()),
        None => Ok(()),
    }
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            "Codex App Server did not respond before the operation timed out".to_string()
        })
}

fn response_id(message: &Value) -> Option<i64> {
    message.get("id").and_then(Value::as_i64)
}

fn response_result(message: Value) -> Result<Value, String> {
    if message.get("error").is_some() {
        return Err("Codex App Server rejected the request".to_string());
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| "Codex App Server response is missing a result".to_string())
}

pub struct TempCodexHome {
    pub path: PathBuf,
    cleanup: bool,
}

impl TempCodexHome {
    pub fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("gswitch-{}", Uuid::new_v4()));
        create_private_directory(&path)?;

        let config_path = path.join("config.toml");
        if let Err(error) = storage::write_private_bytes_atomic(
            &config_path,
            b"cli_auth_credentials_store = \"file\"\n",
            "temporary Codex configuration",
        ) {
            let _ = fs::remove_dir_all(&path);
            return Err(error);
        }

        Ok(Self {
            path,
            cleanup: true,
        })
    }

    pub fn write_auth(&self, credential: &Value) -> Result<(), String> {
        storage::write_json_atomic(
            &self.path.join("auth.json"),
            credential,
            "temporary Codex credentials",
        )
    }

    pub fn read_auth(&self) -> Result<Value, String> {
        storage::read_json(&self.path.join("auth.json"), "temporary Codex credentials")
    }

    pub fn retain_for_recovery(&mut self) {
        self.cleanup = false;
    }
}

impl Drop for TempCodexHome {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| "Unable to create a temporary Codex profile".to_string())?;
    }

    #[cfg(not(unix))]
    {
        fs::create_dir(path)
            .map_err(|_| "Unable to create a temporary Codex profile".to_string())?;
    }

    Ok(())
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
        .ok_or_else(|| "Codex account response is missing an account type".to_string())?;

    let kind = match account_type {
        "chatgpt" => crate::types::AccountKind::ChatGpt,
        "apiKey" | "apikey" => crate::types::AccountKind::ApiKey,
        _ => return Err("This Codex account type is not supported by GSwitch".to_string()),
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
    fn app_server_errors_are_sanitized() {
        let error = response_result(json!({
            "id": 1,
            "error": {"message": "Bearer secret-token must never leak"}
        }))
        .expect_err("error response");
        assert_eq!(error, "Codex App Server rejected the request");
        assert!(!error.contains("secret-token"));
    }

    #[test]
    fn waiting_for_a_message_times_out() {
        let (_sender, receiver) = mpsc::channel();
        let error = receive_message(&receiver, Duration::from_millis(1)).expect_err("timeout");
        assert_eq!(
            error,
            "Codex App Server did not respond before the operation timed out"
        );
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
