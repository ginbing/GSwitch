use std::thread;

use serde_json::{json, Value};

use crate::{
    accounts::AppState,
    app_server::{account_metadata, AppServer, TempCodexHome},
    types::{AccountKind, AccountView, OAuthLoginStart, OAuthLoginStatus},
};

pub fn start_oauth(state: AppState) -> Result<OAuthLoginStart, String> {
    let profile = TempCodexHome::create()?;
    let mut server = AppServer::start(&profile.path)?;

    server.send(json!({
        "method": "account/login/start",
        "id": 1,
        "params": {
            "type": "chatgpt",
            "useHostedLoginSuccessPage": true,
            "appBrand": "chatgpt"
        }
    }))?;

    let result = server.read_response(1)?;
    let login_id = result
        .get("loginId")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex did not return a login id".to_string())?
        .to_string();
    let auth_url = result
        .get("authUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex did not return an authorization URL".to_string())?
        .to_string();

    state.set_oauth_status(login_id.clone(), OAuthLoginStatus::Pending)?;

    let monitor_id = login_id.clone();
    thread::spawn(move || {
        let outcome = monitor_oauth(&mut server, &profile, &state, &monitor_id);
        if let Err(message) = outcome {
            let _ = state.set_oauth_status(monitor_id, OAuthLoginStatus::Failed { message });
        }
    });

    Ok(OAuthLoginStart {
        login_id,
        auth_url,
    })
}

fn monitor_oauth(
    server: &mut AppServer,
    profile: &TempCodexHome,
    state: &AppState,
    login_id: &str,
) -> Result<(), String> {
    loop {
        let message = server.read_message()?;
        if message.get("method").and_then(Value::as_str) != Some("account/login/completed") {
            continue;
        }

        let params = message
            .get("params")
            .ok_or_else(|| "OAuth completion notification is missing params".to_string())?;
        if params.get("loginId").and_then(Value::as_str) != Some(login_id) {
            continue;
        }

        if !params
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let error = params
                .get("error")
                .map(Value::to_string)
                .unwrap_or_else(|| "OAuth login failed".to_string());
            return Err(error);
        }

        let account = server.account_read(2)?;
        let (_, email, plan_type) = account_metadata(&account)?;
        let credential = profile.read_auth()?;
        let label = email
            .clone()
            .unwrap_or_else(|| "ChatGPT account".to_string());
        let view = state.add_account(
            label,
            AccountKind::ChatGpt,
            email,
            plan_type,
            credential,
        )?;
        state.set_oauth_status(
            login_id.to_string(),
            OAuthLoginStatus::Complete { account: view },
        )?;
        return Ok(());
    }
}

pub fn import_json(
    state: &AppState,
    raw_json: &str,
    label: Option<String>,
) -> Result<AccountView, String> {
    let credential: Value =
        serde_json::from_str(raw_json).map_err(|error| format!("Invalid auth JSON: {error}"))?;
    let profile = TempCodexHome::create()?;
    profile.write_auth(&credential)?;

    let mut server = AppServer::start(&profile.path)?;
    let account = server.account_read(1)?;
    let (account_type, email, plan_type) = account_metadata(&account)?;

    let kind = match account_type.as_str() {
        "chatgpt" => AccountKind::ChatGpt,
        "apiKey" | "apikey" => AccountKind::ApiKey,
        other => return Err(format!("Unsupported Codex account type: {other}")),
    };

    let label = label
        .filter(|value| !value.trim().is_empty())
        .or_else(|| email.clone())
        .unwrap_or_else(|| "Imported account".to_string());

    state.add_account(label, kind, email, plan_type, credential)
}

pub fn import_api_key(
    state: &AppState,
    api_key: &str,
    label: Option<String>,
) -> Result<AccountView, String> {
    if api_key.trim().is_empty() {
        return Err("API key is empty".to_string());
    }

    let profile = TempCodexHome::create()?;
    let mut server = AppServer::start(&profile.path)?;
    server.send(json!({
        "method": "account/login/start",
        "id": 1,
        "params": { "type": "apiKey", "apiKey": api_key }
    }))?;
    server.read_response(1)?;

    let account = server.account_read(2)?;
    let (_, email, plan_type) = account_metadata(&account)?;
    let credential = profile.read_auth()?;
    let label = label
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "API key".to_string());

    state.add_account(
        label,
        AccountKind::ApiKey,
        email,
        plan_type,
        credential,
    )
}
