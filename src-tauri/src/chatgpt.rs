use std::{sync::OnceLock, time::Duration};

use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT},
    StatusCode,
};
use serde_json::{json, Value};

use crate::{
    identity::derive_identity,
    types::{AccountIdentity, AccountKind},
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

#[derive(Debug, Clone)]
pub(crate) struct CredentialSnapshot {
    pub(crate) access_token: String,
    pub(crate) account_id: Option<String>,
}

#[derive(Debug)]
pub(crate) struct QuotaResponse {
    pub(crate) usage: Value,
    pub(crate) reset_credit_details: Option<Value>,
    pub(crate) detail_failure: Option<RequestFailure>,
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
    pub(crate) fn new() -> Result<Self, String> {
        Self::with_base_url(CHATGPT_BACKEND_URL)
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: &str) -> Result<Self, String> {
        install_crypto_provider();
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| "Unable to prepare the ChatGPT quota request".to_string())?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn with_base_url(base_url: &str) -> Result<Self, String> {
        install_crypto_provider();
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| "Unable to prepare the ChatGPT quota request".to_string())?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub(crate) fn quota(&self, credential: &Value) -> Result<QuotaResponse, RequestFailure> {
        let snapshot = credential_snapshot(credential).map_err(|_| RequestFailure {
            kind: RequestFailureKind::Authentication,
            status: Some(401),
        })?;
        let usage = self.get_json("/wham/usage", &snapshot)?;
        let (reset_credit_details, detail_failure) =
            match self.get_json("/wham/rate-limit-reset-credits", &snapshot) {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error)),
            };
        Ok(QuotaResponse {
            usage,
            reset_credit_details,
            detail_failure,
        })
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
    pub(crate) fn wake(&self, credential: &Value, model: &str) -> Result<(), RequestFailure> {
        let snapshot = credential_snapshot(credential).map_err(|_| RequestFailure {
            kind: RequestFailureKind::Authentication,
            status: Some(401),
        })?;
        let response = self
            .client
            .post(format!("{}/codex/responses", self.base_url))
            .headers(headers(&snapshot)?)
            .json(&json!({
                "model": model,
                "instructions": "Reply with exactly OK. Do not use tools or inspect files.",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "OK"}]
                }],
                "tools": [],
                "tool_choice": "none",
                "parallel_tool_calls": false,
                "reasoning": {"effort": "none"},
                "store": false,
                "stream": false,
                "include": [],
                "service_tier": "standard"
            }))
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
        Ok(())
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
        .or_else(|| match identity {
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
        let (base_url, handle) = fixture(200, "{}");
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        client
            .wake(&credential(), "gpt-5.6-luna")
            .expect("wake request");

        let request = handle.join().expect("server");
        assert!(request.starts_with("POST /codex/responses HTTP/1.1"));
        assert!(request.contains("authorization: Bearer live-access-token"));
        assert!(request.contains("chatgpt-account-id: workspace"));
        let body = request.split("\r\n\r\n").nth(1).expect("request body");
        let payload: Value = serde_json::from_str(body).expect("JSON payload");
        assert_eq!(payload["model"], "gpt-5.6-luna");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["input"][0]["content"][0]["text"], "OK");
        assert_eq!(payload["tools"], json!([]));
        assert_eq!(payload["tool_choice"], "none");
        assert_eq!(payload["parallel_tool_calls"], false);
        assert_eq!(payload["reasoning"]["effort"], "none");
        assert_eq!(payload["service_tier"], "standard");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], false);
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
