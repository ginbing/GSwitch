use std::{
    io::{BufRead, BufReader, Read},
    sync::OnceLock,
    time::Duration,
};

use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT},
    StatusCode,
};
use serde_json::{json, Value};

use crate::{
    identity::derive_identity,
    types::{AccountIdentity, AccountKind, WakeRequestState},
};

const CHATGPT_BACKEND_URL: &str = "https://chatgpt.com/backend-api";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestFailureKind {
    Authentication,
    RateLimited,
    Http,
    Transport,
    InvalidJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestFailure {
    pub(crate) kind: RequestFailureKind,
    pub(crate) status: Option<u16>,
}

impl RequestFailure {
    pub(crate) fn can_fallback_to_managed_refresh(self) -> bool {
        matches!(self.kind, RequestFailureKind::Authentication)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WakeFailure {
    pub(crate) kind: WakeFailureKind,
    pub(crate) request_state: WakeRequestState,
    pub(crate) status: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeFailureKind {
    Authentication,
    RateLimited,
    ModelUnavailable,
    InvalidRequest,
    ServiceUnavailable,
    Rejected,
    Transport,
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub(crate) struct CredentialSnapshot {
    pub(crate) access_token: String,
    pub(crate) account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountMetadataProjection {
    pub(crate) account_id: String,
    pub(crate) email: Option<String>,
    pub(crate) workspace_name: Option<String>,
    pub(crate) account_structure: Option<String>,
    pub(crate) plan_type: Option<String>,
}

pub(crate) struct ChatGptClient {
    client: Client,
    base_url: String,
}

impl ChatGptClient {
    /// Every production request shares one client and its connection pool, so
    /// after the first account each quota read reuses the open connection to
    /// ChatGPT instead of paying for a new TLS handshake. No cookie store is
    /// enabled, and each request carries only its own account's headers.
    pub(crate) fn new() -> Result<Self, String> {
        static SHARED: OnceLock<Client> = OnceLock::new();
        let client = match SHARED.get() {
            Some(client) => client.clone(),
            None => {
                let client = build_client()?;
                SHARED.get_or_init(|| client).clone()
            }
        };
        Ok(Self {
            client,
            base_url: CHATGPT_BACKEND_URL.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: &str) -> Result<Self, String> {
        Ok(Self {
            client: build_client()?,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub(crate) fn usage(&self, credential: &Value) -> Result<Value, RequestFailure> {
        self.get_json("/wham/usage", &quota_credential(credential)?)
    }

    pub(crate) fn reset_credit_details(&self, credential: &Value) -> Result<Value, RequestFailure> {
        self.get_json(
            "/wham/rate-limit-reset-credits",
            &quota_credential(credential)?,
        )
    }

    pub(crate) fn account_check(&self, credential: &Value) -> Result<Value, RequestFailure> {
        let snapshot = credential_snapshot(credential).map_err(|_| RequestFailure {
            kind: RequestFailureKind::Authentication,
            status: Some(401),
        })?;
        self.get_json("/wham/accounts/check", &snapshot)
    }

    /// Sends the one minimal Responses request used by an explicit Wake.
    ///
    /// This is intentionally not a general Responses client. The request has
    /// no tools, file context, retained state, or retry policy; callers must
    /// treat a transport failure as potentially delivered.
    pub(crate) fn wake(
        &self,
        credential: &Value,
        model: &str,
        reasoning_effort: &str,
    ) -> Result<(), WakeFailure> {
        let snapshot = credential_snapshot(credential).map_err(|_| WakeFailure {
            kind: WakeFailureKind::Authentication,
            request_state: WakeRequestState::NotSent,
            status: None,
        })?;
        let request_headers = headers(&snapshot).map_err(|_| WakeFailure {
            kind: WakeFailureKind::Authentication,
            request_state: WakeRequestState::NotSent,
            status: None,
        })?;
        let response = self
            .client
            .post(format!("{}/codex/responses", self.base_url))
            .headers(request_headers)
            .header(ACCEPT, "text/event-stream")
            // The Codex route uses this client marker for model routing.
            .header("Originator", "codex_cli_rs")
            .json(&json!({
                "model": model,
                "instructions": "",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hi"}]
                }],
                "parallel_tool_calls": true,
                "reasoning": {"effort": reasoning_effort, "summary": "auto"},
                "store": false,
                "stream": true,
                "include": ["reasoning.encrypted_content"]
            }))
            .send()
            .map_err(|_error| WakeFailure {
                kind: WakeFailureKind::Transport,
                request_state: WakeRequestState::MayHaveSent,
                status: None,
            })?;
        let status = response.status();
        if !status.is_success() {
            let kind = if matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ) {
                let mut body = Vec::new();
                let _ = response.take(8_192).read_to_end(&mut body);
                if serde_json::from_slice::<Value>(&body)
                    .is_ok_and(|value| model_unavailable_error(&value))
                {
                    WakeFailureKind::ModelUnavailable
                } else {
                    wake_http_failure_kind(status)
                }
            } else {
                wake_http_failure_kind(status)
            };
            return Err(WakeFailure {
                kind,
                request_state: WakeRequestState::Sent,
                status: Some(status.as_u16()),
            });
        }
        // HTTP success only opens the response stream. A completed turn with
        // assistant text is the proof that this account answered the request.
        parse_wake_stream(BufReader::new(response.take(1_048_576))).map_err(|kind| WakeFailure {
            kind,
            request_state: WakeRequestState::Sent,
            status: None,
        })
    }

    #[cfg(test)]
    fn get_usage(&self, credential: &Value) -> Result<Value, RequestFailure> {
        let snapshot = credential_snapshot(credential).map_err(|_| RequestFailure {
            kind: RequestFailureKind::Authentication,
            status: Some(401),
        })?;
        self.get_json("/wham/usage", &snapshot)
    }

    fn get_json(
        &self,
        path: &str,
        credential: &CredentialSnapshot,
    ) -> Result<Value, RequestFailure> {
        let response = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .headers(headers(credential)?)
            .send()
            .map_err(|_error| RequestFailure {
                kind: RequestFailureKind::Transport,
                status: None,
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(RequestFailure {
                kind: request_failure_kind(status),
                status: Some(status.as_u16()),
            });
        }
        response.json::<Value>().map_err(|_| RequestFailure {
            kind: RequestFailureKind::InvalidJson,
            status: Some(status.as_u16()),
        })
    }
}

fn parse_wake_stream(reader: impl BufRead) -> Result<(), WakeFailureKind> {
    let mut event_name = String::new();
    let mut data = String::new();
    let mut replied = false;
    for line in reader.lines() {
        let line = line.map_err(|_| WakeFailureKind::Transport)?;
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("event:") {
            event_name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        } else if line.is_empty() {
            if data.is_empty() {
                event_name.clear();
                continue;
            }
            let event: Value =
                serde_json::from_str(&data).map_err(|_| WakeFailureKind::InvalidResponse)?;
            let kind = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(&event_name);
            match kind {
                "response.output_item.done" => {
                    replied |= assistant_text(event.get("item"));
                }
                "response.output_text.done" => {
                    replied |= event
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty());
                }
                "response.completed" => {
                    let final_output = event
                        .get("response")
                        .and_then(|response| response.get("output"))
                        .and_then(Value::as_array)
                        .is_some_and(|items| items.iter().any(|item| assistant_text(Some(item))));
                    return if replied || final_output {
                        Ok(())
                    } else {
                        Err(WakeFailureKind::InvalidResponse)
                    };
                }
                "response.failed" | "error" => {
                    return Err(if model_unavailable_error(&event) {
                        WakeFailureKind::ModelUnavailable
                    } else {
                        stream_error_kind(&event)
                    });
                }
                "response.incomplete" => return Err(WakeFailureKind::InvalidResponse),
                _ => {}
            }
            data.clear();
            event_name.clear();
        }
    }
    Err(WakeFailureKind::InvalidResponse)
}

fn wake_http_failure_kind(status: StatusCode) -> WakeFailureKind {
    match status {
        StatusCode::UNAUTHORIZED => WakeFailureKind::Authentication,
        StatusCode::TOO_MANY_REQUESTS => WakeFailureKind::RateLimited,
        StatusCode::BAD_REQUEST => WakeFailureKind::InvalidRequest,
        status if status.is_server_error() => WakeFailureKind::ServiceUnavailable,
        _ => WakeFailureKind::Rejected,
    }
}

fn stream_error_kind(event: &Value) -> WakeFailureKind {
    let error = event
        .get("error")
        .or_else(|| {
            event
                .get("response")
                .and_then(|response| response.get("error"))
        })
        .unwrap_or(event);
    match error.get("code").and_then(Value::as_str) {
        Some("invalid_api_key" | "token_expired" | "unauthorized") => {
            WakeFailureKind::Authentication
        }
        Some("rate_limit_exceeded" | "insufficient_quota") => WakeFailureKind::RateLimited,
        Some("invalid_request_error") => WakeFailureKind::InvalidRequest,
        Some("server_error" | "service_unavailable") => WakeFailureKind::ServiceUnavailable,
        _ => WakeFailureKind::Rejected,
    }
}

fn model_unavailable_error(value: &Value) -> bool {
    let error = value
        .get("error")
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
        })
        .unwrap_or(value);
    if ["code", "type"].iter().any(|field| {
        matches!(
            error.get(*field).and_then(Value::as_str),
            Some(
                "model_not_found"
                    | "model_unavailable"
                    | "model_not_supported"
                    | "unsupported_model"
            )
        )
    }) {
        return true;
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    message.contains("model")
        && [
            "not supported",
            "not available",
            "not found",
            "do not have access",
        ]
        .iter()
        .any(|phrase| message.contains(phrase))
}

fn assistant_text(item: Option<&Value>) -> bool {
    item.is_some_and(|item| {
        item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("role").and_then(Value::as_str) == Some("assistant")
            && item
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part.get("type").and_then(Value::as_str) == Some("output_text")
                            && part
                                .get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.trim().is_empty())
                    })
                })
    })
}

fn quota_credential(credential: &Value) -> Result<CredentialSnapshot, RequestFailure> {
    credential_snapshot(credential).map_err(|_| RequestFailure {
        kind: RequestFailureKind::Authentication,
        status: Some(401),
    })
}

fn build_client() -> Result<Client, String> {
    install_crypto_provider();
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| "Unable to prepare the ChatGPT quota request".to_string())
}

fn install_crypto_provider() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    let _ = INSTALLED.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub(crate) fn credential_snapshot(credential: &Value) -> Result<CredentialSnapshot, String> {
    let identity = derive_identity(&AccountKind::ChatGpt, credential).map_err(|_| {
        "The saved account credentials do not identify a ChatGPT account".to_string()
    })?;
    let tokens = credential.get("tokens").and_then(Value::as_object);
    let access_token = tokens
        .and_then(|tokens| tokens.get("access_token"))
        .or_else(|| credential.get("access_token"))
        .or_else(|| credential.get("accessToken"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "The saved account has no live ChatGPT access token".to_string())?;
    let account_id = tokens
        .and_then(|tokens| tokens.get("account_id"))
        .or_else(|| credential.get("account_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .or(match identity {
            AccountIdentity::ChatGpt { workspace_id, .. } => workspace_id,
            AccountIdentity::ApiKey { .. } => None,
        });
    Ok(CredentialSnapshot {
        access_token: access_token.to_string(),
        account_id,
    })
}

pub(crate) fn validate_response_identity(
    credential: &Value,
    expected: &AccountIdentity,
    response: &Value,
) -> Result<(), String> {
    let actual = derive_identity(&AccountKind::ChatGpt, credential)?;
    if &actual != expected {
        return Err(
            "The saved account credentials do not match their recorded identity".to_string(),
        );
    }
    if let AccountIdentity::ChatGpt {
        user_id,
        workspace_id,
    } = expected
    {
        if let Some(response_user_id) = string_at(response, &["user_id", "userId"]) {
            if response_user_id != *user_id {
                return Err("ChatGPT returned a different account identity".to_string());
            }
        }
        if let Some(response_account_id) = string_at(response, &["account_id", "accountId"]) {
            if workspace_id.as_deref() != Some(response_account_id.as_str()) {
                return Err("ChatGPT returned a different workspace identity".to_string());
            }
        }
    }
    Ok(())
}

pub(crate) fn normalize_account_metadata(
    credential: &Value,
    expected: &AccountIdentity,
    response: &Value,
) -> Result<AccountMetadataProjection, String> {
    let AccountIdentity::ChatGpt { workspace_id, .. } = expected else {
        return Err("Account metadata is available only for ChatGPT accounts".to_string());
    };
    if derive_identity(&AccountKind::ChatGpt, credential)? != *expected {
        return Err(
            "The saved account credentials do not match their recorded identity".to_string(),
        );
    }

    let mut entries = Vec::new();
    if let Some(accounts) = response.get("accounts").and_then(Value::as_array) {
        for entry in accounts.iter().filter(|value| value.is_object()) {
            if let Some(id) = string_at(entry, &["id", "account_id", "accountId"]) {
                entries.push((id, entry));
            }
        }
    } else if let Some(accounts) = response.get("accounts").and_then(Value::as_object) {
        for (map_id, value) in accounts {
            let account = value.get("account").unwrap_or(value);
            let id = string_at(account, &["account_id", "accountId", "id"])
                .unwrap_or_else(|| map_id.to_string());
            entries.push((id, account));
        }
    }

    let selected = match workspace_id.as_deref() {
        Some(workspace_id) => entries
            .iter()
            .find(|(id, _)| id == workspace_id)
            .map(|(id, value)| (id.clone(), *value)),
        None => response
            .get("default_account_id")
            .and_then(Value::as_str)
            .and_then(|default_id| {
                entries
                    .iter()
                    .find(|(id, _)| id == default_id)
                    .map(|(id, value)| (id.clone(), *value))
            })
            .or_else(|| {
                (entries.len() == 1).then(|| {
                    let (id, value) = &entries[0];
                    (id.clone(), *value)
                })
            }),
    }
    .ok_or_else(|| "ChatGPT did not return the expected account workspace".to_string())?;

    Ok(AccountMetadataProjection {
        account_id: selected.0,
        email: crate::identity::email_from_credential(credential),
        workspace_name: string_at(selected.1, &["name", "workspace_name", "account_name"]),
        account_structure: string_at(selected.1, &["structure", "account_structure"]),
        plan_type: string_at(selected.1, &["plan_type", "planType"]),
    })
}

pub(crate) fn merge_reset_credit_details(usage: &Value, details: Option<&Value>) -> Value {
    let mut merged = usage.clone();
    let Some(details) = details else {
        return merged;
    };
    let detail_summary = details
        .get("rate_limit_reset_credits")
        .or_else(|| details.get("rateLimitResetCredits"))
        .filter(|value| value.is_object())
        .cloned()
        .or_else(|| details.is_object().then(|| details.clone()));
    let Some(detail_summary) = detail_summary else {
        return merged;
    };
    let mut summary = merged
        .get("rate_limit_reset_credits")
        .or_else(|| merged.get("rateLimitResetCredits"))
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (summary.as_object_mut(), detail_summary.as_object()) {
        for key in ["available_count", "availableCount", "credits"] {
            if let Some(value) = source.get(key) {
                target.insert(key.to_string(), value.clone());
            }
        }
    }
    if let Some(object) = merged.as_object_mut() {
        object.insert("rate_limit_reset_credits".to_string(), summary);
    }
    merged
}

fn headers(credential: &CredentialSnapshot) -> Result<HeaderMap, RequestFailure> {
    let mut headers = HeaderMap::new();
    let auth =
        HeaderValue::from_str(&format!("Bearer {}", credential.access_token)).map_err(|_| {
            RequestFailure {
                kind: RequestFailureKind::Authentication,
                status: Some(401),
            }
        })?;
    headers.insert(AUTHORIZATION, auth);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let user_agent = HeaderValue::from_static(concat!("GSwitch/", env!("CARGO_PKG_VERSION")));
    headers.insert(USER_AGENT, user_agent);
    if let Some(account_id) = credential.account_id.as_deref() {
        if let Ok(value) = HeaderValue::from_str(account_id) {
            headers.insert("ChatGPT-Account-Id", value);
        }
    }
    Ok(headers)
}

fn request_failure_kind(status: StatusCode) -> RequestFailureKind {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => RequestFailureKind::Authentication,
        StatusCode::TOO_MANY_REQUESTS => RequestFailureKind::RateLimited,
        _ => RequestFailureKind::Http,
    }
}

fn string_at(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use serde_json::json;

    use super::*;

    fn credential() -> Value {
        json!({
            "tokens": {
                "access_token": "live-access-token",
                "account_id": "workspace"
            },
            "auth_mode": "chatgpt",
            "id_token": "eyJhbGciOiJub25lIn0.eyJjaGF0Z3B0X3VzZXJfaWQiOiJ1c2VyIn0.x"
        })
    }

    fn fixture(status: u16, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0; 4096];
            let size = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            request
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn sends_a_read_only_usage_request_with_the_live_token_snapshot() {
        let (base_url, handle) = fixture(200, r#"{"plan_type":"plus"}"#);
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let response = client.get_usage(&credential()).expect("usage");
        assert_eq!(response["plan_type"], "plus");
        let request = handle.join().expect("server");
        assert!(request.starts_with("GET /wham/usage HTTP/1.1"));
        assert!(request.contains("authorization: Bearer live-access-token"));
        assert!(request.contains("chatgpt-account-id: workspace"));
    }

    #[test]
    fn uses_the_current_accounts_check_route_for_read_only_metadata() {
        let (base_url, handle) = fixture(
            200,
            r#"{"accounts":[{"id":"workspace","name":"Personal","structure":"workspace","plan_type":"plus"}]}"#,
        );
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let response = client.account_check(&credential()).expect("accounts");
        assert_eq!(response["accounts"][0]["id"], "workspace");
        let request = handle.join().expect("server");
        assert!(request.starts_with("GET /wham/accounts/check HTTP/1.1"));
    }

    #[test]
    fn routes_an_account_check_to_the_workspace_claim_when_account_id_is_absent() {
        let claims = json!({
            "https://api.openai.com/auth": {
                "chatgpt_user_id": "user",
                "chatgpt_account_id": "selected-workspace"
            }
        });
        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"))
        );
        let credential = json!({"tokens": {
            "id_token": id_token,
            "access_token": "live-access-token"
        }});
        let (base_url, handle) = fixture(200, r#"{"accounts":[]}"#);
        ChatGptClient::with_base_url(&base_url)
            .expect("client")
            .account_check(&credential)
            .expect("account check");
        let request = handle.join().expect("server");
        assert!(request.contains("chatgpt-account-id: selected-workspace"));
    }

    #[test]
    fn sends_one_minimal_codex_responses_wake_request() {
        let response = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi!\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\"}}\n\n"
        );
        let (base_url, handle) = fixture(200, response);
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        client
            .wake(&credential(), "gpt-6-luna", "low")
            .expect("wake request");

        let request = handle.join().expect("server");
        assert!(request.starts_with("POST /codex/responses HTTP/1.1"));
        assert!(request.contains("authorization: Bearer live-access-token"));
        assert!(request.contains("chatgpt-account-id: workspace"));
        assert!(request.contains("originator: codex_cli_rs"));
        let body = request.split("\r\n\r\n").nth(1).expect("request body");
        let payload: Value = serde_json::from_str(body).expect("JSON payload");
        assert_eq!(payload["model"], "gpt-6-luna");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["input"][0]["content"][0]["text"], "hi");
        assert_eq!(payload["instructions"], "");
        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
        assert_eq!(payload["parallel_tool_calls"], true);
        assert_eq!(payload["reasoning"]["effort"], "low");
        assert_eq!(payload["reasoning"]["summary"], "auto");
        assert!(payload.get("service_tier").is_none());
        assert_eq!(payload["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn wake_requires_a_completed_turn_and_assistant_reply() {
        for body in [
            "{}",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1\",\"output\":[]}}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"OK\"}]}}\n\n",
        ] {
            let (base_url, handle) = fixture(200, body);
            let client = ChatGptClient::with_base_url(&base_url).expect("client");
            let failure = client.wake(&credential(), "gpt-6-luna", "low").expect_err("not confirmed");
            assert_eq!(failure.kind, WakeFailureKind::InvalidResponse);
            assert_eq!(failure.request_state, WakeRequestState::Sent);
            handle.join().expect("server");
        }
    }

    #[test]
    fn wake_keeps_provider_rejection_distinct_from_an_unconfirmed_reply() {
        let (base_url, handle) = fixture(200,
            "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-1\"}}\n\n");
        let failure = ChatGptClient::with_base_url(&base_url)
            .expect("client")
            .wake(&credential(), "gpt-6-luna", "low")
            .expect_err("rejected");
        assert_eq!(failure.kind, WakeFailureKind::Rejected);
        assert_eq!(failure.request_state, WakeRequestState::Sent);
        assert_eq!(failure.status, None);
        handle.join().expect("server");
    }

    #[test]
    fn offers_a_model_retry_only_for_an_explicit_model_rejection() {
        for (status, body, expected) in [
            (
                400,
                r#"{"error":{"code":"model_not_found","message":"Unknown model"}}"#,
                WakeFailureKind::ModelUnavailable,
            ),
            (
                400,
                r#"{"error":{"message":"The model is not supported for this account"}}"#,
                WakeFailureKind::ModelUnavailable,
            ),
            (
                400,
                r#"{"error":{"code":"invalid_request_error","message":"Bad request"}}"#,
                WakeFailureKind::InvalidRequest,
            ),
            (
                403,
                r#"{"error":{"message":"Forbidden"}}"#,
                WakeFailureKind::Rejected,
            ),
            (
                503,
                r#"{"error":{"message":"Unavailable"}}"#,
                WakeFailureKind::ServiceUnavailable,
            ),
        ] {
            let (base_url, handle) = fixture(status, body);
            let failure = ChatGptClient::with_base_url(&base_url)
                .expect("client")
                .wake(&credential(), "gpt-6-luna", "low")
                .expect_err("rejected");
            assert_eq!(failure.kind, expected);
            assert_eq!(failure.request_state, WakeRequestState::Sent);
            assert_eq!(failure.status, Some(status));
            handle.join().expect("server");
        }
    }

    #[test]
    fn recognizes_an_explicit_model_error_in_the_response_stream() {
        let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"model_not_found\",\"message\":\"Unavailable\"}}\n\n";
        let (base_url, handle) = fixture(200, body);
        let failure = ChatGptClient::with_base_url(&base_url)
            .expect("client")
            .wake(&credential(), "gpt-6-luna", "low")
            .expect_err("model rejected");
        assert_eq!(failure.kind, WakeFailureKind::ModelUnavailable);
        assert_eq!(failure.request_state, WakeRequestState::Sent);
        handle.join().expect("server");
    }

    #[test]
    fn classifies_stream_errors_without_exposing_provider_messages() {
        for (code, expected) in [
            ("invalid_request_error", WakeFailureKind::InvalidRequest),
            ("rate_limit_exceeded", WakeFailureKind::RateLimited),
            ("server_error", WakeFailureKind::ServiceUnavailable),
        ] {
            let body = format!(
                "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"code\":\"{code}\",\"message\":\"private provider details\"}}}}\n\n"
            );
            let (base_url, handle) = fixture(200, &body);
            let failure = ChatGptClient::with_base_url(&base_url)
                .expect("client")
                .wake(&credential(), "gpt-6-luna", "low")
                .expect_err("stream rejected");
            assert_eq!(failure.kind, expected);
            assert_eq!(failure.request_state, WakeRequestState::Sent);
            assert_eq!(failure.status, None);
            handle.join().expect("server");
        }
    }

    #[test]
    fn alternate_model_keeps_the_same_stateless_greeting() {
        let (base_url, handle) = fixture(400, "{}");
        let _ = ChatGptClient::with_base_url(&base_url)
            .expect("client")
            .wake(&credential(), "gpt-5.6-luna", "low");
        let request = handle.join().expect("server");
        let body = request.split("\r\n\r\n").nth(1).expect("request body");
        let payload: Value = serde_json::from_str(body).expect("JSON payload");
        assert_eq!(payload["model"], "gpt-5.6-luna");
        assert_eq!(payload["reasoning"]["effort"], "low");
        assert_eq!(payload["input"][0]["content"][0]["text"], "hi");
        assert_eq!(payload["store"], false);
    }

    #[test]
    fn only_authentication_failures_are_refresh_candidates() {
        let (base_url, handle) = fixture(401, "{}");
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let error = client.get_usage(&credential()).expect_err("auth failure");
        assert_eq!(error.kind, RequestFailureKind::Authentication);
        assert!(error.can_fallback_to_managed_refresh());
        handle.join().expect("server");

        let (base_url, handle) = fixture(429, "{}");
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let error = client.get_usage(&credential()).expect_err("rate limit");
        assert_eq!(error.kind, RequestFailureKind::RateLimited);
        assert!(!error.can_fallback_to_managed_refresh());
        handle.join().expect("server");

        let (base_url, handle) = fixture(500, "{}");
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let error = client.get_usage(&credential()).expect_err("server error");
        assert_eq!(error.kind, RequestFailureKind::Http);
        assert!(!error.can_fallback_to_managed_refresh());
        handle.join().expect("server");

        let (base_url, handle) = fixture(200, "not-json");
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let error = client.get_usage(&credential()).expect_err("parse error");
        assert_eq!(error.kind, RequestFailureKind::InvalidJson);
        assert!(!error.can_fallback_to_managed_refresh());
        handle.join().expect("server");
    }

    #[test]
    fn merges_detail_rows_without_exposing_the_provider_identifier_to_the_view() {
        let usage = json!({
            "rate_limit_reset_credits": {"available_count": 2}
        });
        let details = json!({
            "credits": [{"id": "opaque", "status": "available"}]
        });
        let merged = merge_reset_credit_details(&usage, Some(&details));
        assert_eq!(merged["rate_limit_reset_credits"]["available_count"], 2);
        assert_eq!(
            merged["rate_limit_reset_credits"]["credits"][0]["id"],
            "opaque"
        );
    }

    #[test]
    fn normalizes_only_the_expected_workspace_from_the_current_accounts_contract() {
        let credential = credential();
        let expected = AccountIdentity::ChatGpt {
            user_id: "user".to_string(),
            workspace_id: Some("workspace".to_string()),
        };
        let response = json!({
            "accounts": [
                {"id": "other", "name": "Other", "structure": "workspace", "plan_type": "free"},
                {"id": "workspace", "name": "Personal", "structure": "workspace", "plan_type": "plus"}
            ],
            "default_account_id": "workspace"
        });
        let metadata =
            normalize_account_metadata(&credential, &expected, &response).expect("metadata");
        assert_eq!(metadata.account_id, "workspace");
        assert_eq!(metadata.workspace_name.as_deref(), Some("Personal"));
        assert_eq!(metadata.account_structure.as_deref(), Some("workspace"));
        assert_eq!(metadata.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn rejects_an_accounts_check_workspace_mismatch() {
        let credential = credential();
        let expected = AccountIdentity::ChatGpt {
            user_id: "user".to_string(),
            workspace_id: Some("workspace".to_string()),
        };
        let error = normalize_account_metadata(
            &credential,
            &expected,
            &json!({"accounts": [{"id": "different", "name": "Wrong"}]}),
        )
        .expect_err("mismatch");
        assert_eq!(
            error,
            "ChatGPT did not return the expected account workspace"
        );
    }
}
