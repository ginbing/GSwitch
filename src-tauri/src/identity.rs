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

pub fn document_fingerprint(credential: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(credential)
        .map_err(|_| "Unable to fingerprint Codex credentials".to_string())?;
    Ok(sha256_hex(&bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub fn email_from_credential(credential: &Value) -> Option<String> {
    let token = credential
        .pointer("/tokens/id_token")
        .or_else(|| credential.get("id_token"))
        .and_then(Value::as_str)?;
    let claims = jwt_claims(token).ok()?;
    let auth = claims
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    string_at(auth, "email")
        .or_else(|| string_at(claims.as_object(), "email"))
        .map(ToString::to_string)
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

    Ok(AccountIdentity::ApiKey {
        fingerprint: sha256_hex(key.as_bytes()),
    })
}

/// Whether the saved access token's own `exp` claim has passed. GSwitch reads
/// it only to skip a provider request that is certain to be rejected; a token
/// without a readable expiry counts as possibly valid.
pub(crate) fn access_token_expired(credential: &Value, now_seconds: i64) -> bool {
    credential
        .get("tokens")
        .and_then(|tokens| tokens.get("access_token"))
        .or_else(|| credential.get("access_token"))
        .or_else(|| credential.get("accessToken"))
        .and_then(Value::as_str)
        .and_then(|token| jwt_claims(token).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64))
        .is_some_and(|expires_at| expires_at <= now_seconds)
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

    fn with_access_token_expiry(expires_at: Option<i64>) -> Value {
        let claims = match expires_at {
            Some(expires_at) => serde_json::json!({"exp": expires_at}),
            None => serde_json::json!({}),
        };
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"))
        );
        serde_json::json!({"tokens": {"access_token": token}})
    }

    #[test]
    fn an_access_token_counts_as_expired_only_when_its_own_expiry_has_passed() {
        assert!(access_token_expired(
            &with_access_token_expiry(Some(100)),
            100
        ));
        assert!(!access_token_expired(
            &with_access_token_expiry(Some(101)),
            100
        ));
        assert!(!access_token_expired(&with_access_token_expiry(None), 100));
        assert!(!access_token_expired(
            &serde_json::json!({"tokens": {"access_token": "opaque"}}),
            100
        ));
        assert!(!access_token_expired(&serde_json::json!({}), 100));
    }
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

    #[test]
    fn sha256_hex_is_lowercase_and_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
