use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    accounts::{AppState, OperationAcquireFailure},
    app_server, runtime,
    types::{CodexCliInfo, CodexCliUpdateFailure, CodexCliUpdateStatus},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_OUTPUT: usize = 32 * 1024;
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";

/// Only the version and presence of the official update subcommand cross IPC.
/// Never return CLI diagnostics, environment values, paths, or command output.
pub fn inspect() -> CodexCliInfo {
    inspect_with(app_server::codex_command)
}

fn inspect_with(mut command: impl FnMut() -> Command) -> CodexCliInfo {
    let version = probe(command(), "--version")
        .as_deref()
        .and_then(parse_version);
    let supports_update = version.is_some()
        && probe(command(), "--help")
            .as_deref()
            .is_some_and(help_lists_update);
    CodexCliInfo {
        update_status: if version.is_some() {
            CodexCliUpdateStatus::Unknown
        } else {
            CodexCliUpdateStatus::Missing
        },
        version,
        supports_update,
        latest_version: None,
    }
}

/// A bounded, credential-free check of the latest stable upstream CLI release.
/// Failure is an unknown status, never an invitation to update blindly.
pub fn check_latest() -> CodexCliInfo {
    let info = inspect();
    if info.version.is_none() {
        return info;
    }
    with_latest(info, latest_stable_version())
}

fn with_latest(mut info: CodexCliInfo, latest: Option<String>) -> CodexCliInfo {
    let Some(installed) = info.version.as_deref() else {
        return info;
    };
    if let Some(latest) = latest {
        info.update_status = if newer_than(&latest, installed) {
            CodexCliUpdateStatus::Available
        } else {
            CodexCliUpdateStatus::Current
        };
        info.latest_version = Some(latest);
    }
    info
}

fn latest_stable_version() -> Option<String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    let response = client
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::USER_AGENT, "GSwitch")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut body = Vec::new();
    response.take(4 * 1024 * 1024).read_to_end(&mut body).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    parse_release_tag(value.get("tag_name")?.as_str()?)
}

fn parse_release_tag(tag: &str) -> Option<String> {
    let tag = tag.strip_prefix("rust-v")?;
    let version = parse_version(&format!("codex-cli {tag}"))?;
    if version.contains('-') {
        return None;
    }
    Some(version)
}

fn newer_than(latest: &str, installed: &str) -> bool {
    let parse = |version: &str| -> Option<([u64; 3], bool)> {
        let mut parts = version.splitn(2, ['-', '+']);
        let core = parts.next()?;
        let numbers = core
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let numbers: [u64; 3] = numbers.try_into().ok()?;
        Some((numbers, version.contains('-')))
    };
    let (Some((latest_core, latest_pre)), Some((installed_core, installed_pre))) =
        (parse(latest), parse(installed))
    else {
        return false;
    };
    latest_core > installed_core || (latest_core == installed_core && installed_pre && !latest_pre)
}

pub fn update(state: &AppState) -> Result<CodexCliInfo, CodexCliUpdateFailure> {
    let _operation = state
        .acquire_cli_update_operation()
        .map_err(|error| match error {
            OperationAcquireFailure::Busy => CodexCliUpdateFailure::Busy,
            OperationAcquireFailure::Failed(_) => CodexCliUpdateFailure::UpdateFailed,
        })?;
    let before = inspect();
    if before.version.is_none() {
        return Err(CodexCliUpdateFailure::NotInstalled);
    }
    if !before.supports_update {
        return Err(CodexCliUpdateFailure::Unsupported);
    }
    runtime::ensure_no_external_codex(&[]).map_err(|_| CodexCliUpdateFailure::CodexOpen)?;

    run_official_update(app_server::codex_command())?;

    let after = inspect();
    if after.version.is_none() {
        return Err(CodexCliUpdateFailure::VerificationFailed);
    }
    Ok(after)
}

fn run_official_update(mut command: Command) -> Result<(), CodexCliUpdateFailure> {
    command
        .arg("update")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let status = command
        .status()
        .map_err(|_| CodexCliUpdateFailure::UpdateFailed)?;
    if !status.success() {
        return Err(CodexCliUpdateFailure::UpdateFailed);
    }
    Ok(())
}

fn probe(mut command: Command, argument: &str) -> Option<String> {
    command
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let mut child = command.spawn().ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = thread::Builder::new()
        .name("gswitch-cli-probe".to_string())
        .spawn(move || {
            let mut captured = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let count = stdout.read(&mut chunk).ok()?;
                if count == 0 {
                    break;
                }
                let keep = count.min(MAX_PROBE_OUTPUT.saturating_sub(captured.len()));
                captured.extend_from_slice(&chunk[..keep]);
            }
            String::from_utf8(captured).ok()
        })
        .ok();
    let Some(reader) = reader else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(30)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    if success {
        reader.join().ok().flatten()
    } else {
        // A launcher may leave a grandchild holding stdout open. Do not make
        // the five-second timeout wait indefinitely for that pipe to close.
        None
    }
}

fn parse_version(output: &str) -> Option<String> {
    let mut words = output.split_whitespace();
    if words.next()? != "codex-cli" {
        return None;
    }
    let version = words.next()?;
    if version.len() > 48
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
    {
        return None;
    }
    let core = version.split(['-', '+']).next()?;
    if core.split('.').count() != 3 || !core.split('.').all(|part| part.parse::<u64>().is_ok()) {
        return None;
    }
    Some(version.to_string())
}

fn help_lists_update(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.split_whitespace().next() == Some("update"))
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_codex_cli_versions_and_an_explicit_update_command() {
        assert_eq!(
            parse_version("codex-cli 0.156.1\n"),
            Some("0.156.1".to_string())
        );
        assert_eq!(
            parse_version("codex-cli 0.157.0-beta.1\n"),
            Some("0.157.0-beta.1".to_string())
        );
        assert_eq!(parse_version("other-cli 0.156.1\n"), None);
        assert_eq!(parse_version("codex-cli not-a-version\n"), None);
        assert!(help_lists_update(
            "Commands:\n  update  Update Codex to the latest version\n"
        ));
        assert!(!help_lists_update("Commands:\n  exec  Run Codex\n"));
    }

    #[test]
    fn checks_only_valid_stable_releases_and_does_not_guess_when_offline() {
        assert_eq!(parse_release_tag("rust-v0.158.0"), Some("0.158.0".into()));
        assert_eq!(parse_release_tag("rust-v0.158.0-beta.1"), None);
        assert_eq!(parse_release_tag("other-v0.158.0"), None);
        let info = CodexCliInfo {
            version: Some("0.157.0".into()),
            supports_update: true,
            latest_version: None,
            update_status: CodexCliUpdateStatus::Unknown,
        };
        assert_eq!(
            with_latest(info.clone(), None).update_status,
            CodexCliUpdateStatus::Unknown
        );
        assert_eq!(
            with_latest(info.clone(), Some("0.157.0".into())).update_status,
            CodexCliUpdateStatus::Current
        );
        assert_eq!(
            with_latest(info, Some("0.158.0".into())).update_status,
            CodexCliUpdateStatus::Available
        );
        assert!(newer_than("0.158.0", "0.158.0-beta.1"));
        assert!(!newer_than("0.157.0", "0.158.0-beta.1"));
    }

    #[cfg(windows)]
    #[test]
    fn inspects_a_fake_windows_cli_without_running_its_update_action() {
        let root =
            std::env::temp_dir().join(format!("gswitch-cli-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("fixture dir");
        let script = root.join("codex.cmd");
        std::fs::write(
            &script,
            "@echo off\r\nif \"%1\"==\"--version\" echo codex-cli 0.156.1\r\nif \"%1\"==\"--help\" echo   update  Update Codex\r\nif \"%1\"==\"update\" echo called>\"%~dp0updated.txt\"\r\n",
        )
        .expect("fixture script");
        let info = inspect_with(|| Command::new(&script));
        assert_eq!(info.version.as_deref(), Some("0.156.1"));
        assert!(info.supports_update);
        assert!(!root.join("updated.txt").exists());
        run_official_update(Command::new(&script)).expect("fake CLI update");
        assert!(root.join("updated.txt").exists());
        std::fs::remove_dir_all(root).expect("remove fixture");
    }
}
