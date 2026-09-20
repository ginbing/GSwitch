use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::types::{AccountIdentity, AccountKind};

pub fn document_kind(credential: &Value) -> Result<AccountKind, String> {
    if credential
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
    {
        return Ok(AccountKind::ApiKey);
    }

    if credential
        .get("tokens")
        .and_then(Value::as_object)
        .is_some()
        || credential
            .get("auth_mode")
            .and_then(Value::as_str)
            .is_some_and(|mode| mode.eq_ignore_ascii_case("chatgpt"))
    {
        return Ok(AccountKind::ChatGpt);
    }

    Err("The credential document does not contain a supported Codex account".to_string())
}

pub fn derive_identity(kind: &AccountKind, credential: &Value) -> Result<AccountIdentity, String> {
    match kind {
        AccountKind::ChatGpt => chatgpt_identity(credential),
        AccountKind::ApiKey => api_key_identity(credential),
    }
}

fn chatgpt_identity(credential: &Value) -> Result<AccountIdentity, String> {
    let token = credential
        .pointer("/tokens/id_token")
        .or_else(|| credential.get("id_token"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "The ChatGPT credential does not contain a stable identity token".to_string()
        })?;

    let claims = jwt_claims(token)?;
    let auth = claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);

    let user_id = string_at(auth, "chatgpt_user_id")
        .or_else(|| string_at(auth, "user_id"))
        .or_else(|| string_at(claims.as_object(), "chatgpt_user_id"))
        .or_else(|| string_at(claims.as_object(), "user_id"))
        .ok_or_else(|| {
            "The ChatGPT credential does not contain a stable user identity".to_string()
        })?;

    let workspace_id = string_at(auth, "chatgpt_account_id")
        .or_else(|| string_at(auth, "workspace_id"))
        .or_else(|| {
            credential
                .pointer("/tokens/account_id")
                .and_then(Value::as_str)
        })
        .map(ToString::to_string);

    Ok(AccountIdentity::ChatGpt {
        user_id: user_id.to_string(),
        workspace_id,
    })
}

fn api_key_identity(credential: &Value) -> Result<AccountIdentity, String> {
    let key = credential
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "The API-key credential does not contain an API key".to_string())?;

    let digest = Sha256::digest(key.as_bytes());
    Ok(AccountIdentity::ApiKey {
        fingerprint: format!("{digest:x}"),
    })
}

fn jwt_claims(token: &str) -> Result<Value, String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "The ChatGPT identity token is malformed".to_string())?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "The ChatGPT identity token is malformed".to_string())?;
    serde_json::from_slice(&bytes)
        .map_err(|_| "The ChatGPT identity token is malformed".to_string())
}

fn string_at<'a>(object: Option<&'a serde_json::Map<String, Value>>, key: &str) -> Option<&'a str> {
    object
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    fn id_token(user: &str, workspace: &str) -> String {
        let payload = json!({
            "https://api.openai.com/auth": {
                "chatgpt_user_id": user,
                "chatgpt_account_id": workspace
            }
        });
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload"))
        )
    }

    #[test]
    fn chatgpt_identity_includes_workspace() {
        let credential = json!({"tokens": {"id_token": id_token("user-1", "workspace-1")}});
        assert_eq!(
            derive_identity(&AccountKind::ChatGpt, &credential).expect("identity"),
            AccountIdentity::ChatGpt {
                user_id: "user-1".into(),
                workspace_id: Some("workspace-1".into())
            }
        );
    }

    #[test]
    fn api_key_identity_is_a_non_secret_fingerprint() {
        let identity = derive_identity(
            &AccountKind::ApiKey,
            &json!({"OPENAI_API_KEY": "sk-secret"}),
        )
        .expect("identity");
        let AccountIdentity::ApiKey { fingerprint } = identity else {
            panic!("expected API key identity");
        };
        assert_eq!(fingerprint.len(), 64);
        assert!(!fingerprint.contains("sk-secret"));
    }
}
