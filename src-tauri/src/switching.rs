use serde_json::{json, Value};

use crate::{
    accounts::{AppState, OperationGuard},
    app_server::{account_metadata, AccountMetadata, AppServer, TempCodexHome},
    codex,
    identity::{derive_identity, document_fingerprint, document_kind},
    runtime,
    types::{
        AccountIdentity, AccountKind, AccountView, CredentialStoreMode, LiveAccountStatus,
        LiveAccountView, PendingSwitch, PendingSwitchStage, StoredAccount, SwitchOutcome,
    },
};

const CONFIG_STORE_KEY: &str = "cli_auth_credentials_store";

pub fn live_account(state: &AppState) -> Result<LiveAccountView, String> {
    if state.recovery_required() {
        return Ok(LiveAccountView {
            status: LiveAccountStatus::RecoveryRequired,
            credential_store: CredentialStoreMode::Unknown,
            account: None,
            message: Some(
                "GSwitch account storage needs recovery before it can inspect or change Codex accounts"
                    .into(),
            ),
        });
    }

    let codex_home = codex::codex_home()?;
    let credential_store = codex::credential_store_mode(&codex_home)?;

    if state.has_pending_switch()? {
        return Ok(LiveAccountView {
            status: LiveAccountStatus::RecoveryRequired,
            credential_store,
            account: None,
            message: Some(
                "A previous switch needs recovery before GSwitch can change accounts".into(),
            ),
        });
    }

    if credential_store != CredentialStoreMode::File {
        return Ok(LiveAccountView {
            status: LiveAccountStatus::FileStoreRequired,
            credential_store,
            account: None,
            message: Some("Enable file-backed Codex credentials before switching accounts".into()),
        });
    }

    let Some(credential) = codex::read_optional_auth_document(&codex_home)? else {
        return Ok(LiveAccountView {
            status: LiveAccountStatus::NotSignedIn,
            credential_store,
            account: None,
            message: None,
        });
    };
    let kind = document_kind(&credential)?;
    let identity = derive_identity(&kind, &credential)?;
    let Some(mut account) = state.account_by_identity(&identity)? else {
        return Ok(LiveAccountView {
            status: LiveAccountStatus::UnknownAccount,
            credential_store,
            account: None,
            message: Some("Save the current Codex account before replacing its credentials".into()),
        });
    };
    account.active = true;
    Ok(LiveAccountView {
        status: LiveAccountStatus::Ready,
        credential_store,
        account: Some(account),
        message: None,
    })
}

pub fn save_current_account(state: &AppState) -> Result<AccountView, String> {
    let operation = state.acquire_operation()?;
    ensure_ready_for_credential_operation(state, &operation)?;
    runtime::ensure_no_external_codex(&[])?;

    let codex_home = codex::codex_home()?;
    let mut server = start_effective_file_store(&codex_home)?;
    let original_credential = codex::read_optional_auth_document(&codex_home)?
        .ok_or_else(|| "Codex is not signed in with a file-backed credential".to_string())?;
    let kind = document_kind(&original_credential)?;
    let identity = derive_identity(&kind, &original_credential)?;
    let metadata = account_metadata(&server.account_read(2, false)?)?;
    ensure_metadata_kind(&metadata, &kind)?;
    let credential = codex::read_auth_document(&codex_home)?;
    if document_kind(&credential)? != kind || derive_identity(&kind, &credential)? != identity {
        return Err("Codex credentials changed while saving the current account".to_string());
    }
    let default_label = metadata.email.clone().unwrap_or_else(|| match kind {
        AccountKind::ChatGpt => "Current Codex account".to_string(),
        AccountKind::ApiKey => "Current API key".to_string(),
    });
    let recovery_credential = credential.clone();

    let result = state.upsert_under_operation(
        &operation,
        crate::accounts::AccountDraft {
            label: None,
            default_label,
            kind,
            email: metadata.email,
            plan_type: metadata.plan_type,
            identity,
            credential,
        },
    );
    if result.is_err() {
        let _ = state.record_pending_credential(&operation, &recovery_credential);
        return Err(
            "GSwitch could not save the current account. A protected recovery copy was retained."
                .to_string(),
        );
    }
    result
}

/// Changes only Codex's official setting. It never extracts a keyring or
/// ephemeral credential. The current file credential must already be a saved
/// GSwitch account, so a failed configuration write cannot orphan it.
pub fn enable_file_store(state: &AppState) -> Result<bool, String> {
    let operation = state.acquire_operation()?;
    ensure_ready_for_credential_operation(state, &operation)?;
    runtime::ensure_no_external_codex(&[])?;

    let codex_home = codex::codex_home()?;
    let mode = codex::credential_store_mode(&codex_home)?;
    if mode == CredentialStoreMode::File {
        return Ok(false);
    }
    if matches!(
        mode,
        CredentialStoreMode::Keyring | CredentialStoreMode::Ephemeral
    ) {
        return Err("GSwitch cannot export Codex keyring or ephemeral credentials. Sign in again with a file-backed account, then enable switching.".to_string());
    }
    if mode == CredentialStoreMode::Unknown {
        return Err("Codex credential storage mode is not supported by GSwitch".to_string());
    }

    let credential = codex::read_optional_auth_document(&codex_home)?.ok_or_else(|| {
        "GSwitch needs an existing file-backed Codex credential before it can enable switching"
            .to_string()
    })?;
    let kind = document_kind(&credential)?;
    let identity = derive_identity(&kind, &credential)?;
    let saved = state
        .account_by_identity_under_operation(&operation, &identity)?
        .ok_or_else(|| "Save the current Codex account before enabling switching".to_string())?;

    let mut server = AppServer::start(&codex_home)?;
    runtime::ensure_no_external_codex(&[server.pid()])?;
    let config = server.config_read(1)?;
    reject_managed_store_origin(&config)?;
    let expected_version = origin_version(&config).ok_or_else(|| {
        "Codex did not provide a configuration version for safe updating".to_string()
    })?;
    runtime::ensure_no_external_codex(&[server.pid()])?;
    server.config_value_write(2, CONFIG_STORE_KEY, json!("file"), Some(expected_version))?;
    let verified_config = server.config_read(3)?;
    reject_managed_store_origin(&verified_config)?;
    if config_store_value(&verified_config) != Some("file") {
        return Err("Codex did not confirm file-backed credential storage".to_string());
    }
    runtime::ensure_no_external_codex(&[server.pid()])?;

    let refreshed = codex::read_optional_auth_document(&codex_home)?.ok_or_else(|| {
        "Codex credentials disappeared while enabling account switching".to_string()
    })?;
    if document_kind(&refreshed)? != kind || derive_identity(&kind, &refreshed)? != identity {
        return Err("Codex credentials changed while enabling account switching".to_string());
    }
    update_credential_or_record(state, &operation, &saved.id, refreshed)?;
    Ok(true)
}

pub fn switch_account(state: &AppState, target_id: &str) -> Result<SwitchOutcome, String> {
    let operation = state.acquire_operation()?;
    ensure_ready_for_credential_operation(state, &operation)?;
    runtime::ensure_no_external_codex(&[])?;

    let codex_home = codex::codex_home()?;
    drop(start_effective_file_store(&codex_home)?);
    let target = state.account_by_id_under_operation(&operation, target_id)?;
    let (target_kind, target_identity) = stored_identity(&target)?;

    let previous_auth = codex::read_optional_auth_document(&codex_home)?;
    let previous_fingerprint = previous_auth
        .as_ref()
        .map(document_fingerprint)
        .transpose()?;
    if let Some(current) = previous_auth.as_ref() {
        let (_, current_identity) = credential_identity(current)?;
        let current_saved = state
            .account_by_identity_under_operation(&operation, &current_identity)?
            .ok_or_else(|| {
                "Save the current Codex account before replacing its credentials".to_string()
            })?;
        if current_identity == target_identity {
            return confirm_already_active(
                state,
                &operation,
                &codex_home,
                &target,
                &target_kind,
                &target_identity,
            );
        }
        update_credential_or_record(state, &operation, &current_saved.id, current.clone())?;
    }

    // Never refresh or alter the selected profile until the live credential is
    // known to be safe to replace.
    let validated_credential =
        validate_target_in_isolation(state, &operation, &target, &target_kind, &target_identity)?;

    let pending = PendingSwitch {
        target_id: target.id.clone(),
        target_identity: target_identity.clone(),
        previous_active_id: state.active_account_id_under_operation(&operation)?,
        previous_auth: previous_auth.clone(),
        stage: PendingSwitchStage::Prepared,
    };
    state.prepare_switch_under_operation(&operation, pending)?;

    // A process could start or a user could edit auth.json after validation.
    // Both checks must pass before this is allowed to replace the live file.
    runtime::ensure_no_external_codex(&[])?;
    let observed = codex::read_optional_auth_document(&codex_home)?;
    if observed.as_ref().map(document_fingerprint).transpose()? != previous_fingerprint {
        return Err("Codex credentials changed while GSwitch was preparing the switch".to_string());
    }
    codex::write_auth_document(&codex_home, &validated_credential)?;

    match verify_written_target(
        state,
        &operation,
        &codex_home,
        &target,
        &target_kind,
        &target_identity,
    ) {
        Ok(account) => Ok(SwitchOutcome { account }),
        Err(error) => match restore_previous_if_unchanged(
            state,
            &operation,
            &codex_home,
            &validated_credential,
            previous_auth,
        ) {
            Ok(()) => Err(error),
            Err(recovery_error) => Err(format!("{error}. {recovery_error}")),
        },
    }
}

pub fn recover_pending_switch(state: &AppState) -> Result<(), String> {
    let operation = state.acquire_operation()?;
    let Some(pending) = state.pending_switch_under_operation(&operation)? else {
        return Ok(());
    };
    runtime::ensure_no_external_codex(&[])?;
    let codex_home = codex::codex_home()?;
    drop(start_effective_file_store(&codex_home)?);
    let current = codex::read_optional_auth_document(&codex_home)?;

    if let Some(current) = current {
        let (kind, identity) = credential_identity(&current)?;
        if kind == account_kind_for_identity(&pending.target_identity)
            && identity == pending.target_identity
        {
            let target = state.account_by_id_under_operation(&operation, &pending.target_id)?;
            let (target_kind, target_identity) = stored_identity(&target)?;
            return verify_written_target(
                state,
                &operation,
                &codex_home,
                &target,
                &target_kind,
                &target_identity,
            )
            .map(|_| ());
        }
        if pending
            .previous_auth
            .as_ref()
            .map(document_fingerprint)
            .transpose()?
            == Some(document_fingerprint(&current)?)
        {
            state.clear_pending_switch_under_operation(&operation, pending.previous_active_id)?;
            return Ok(());
        }
    } else if pending.previous_auth.is_none() {
        state.clear_pending_switch_under_operation(&operation, pending.previous_active_id)?;
        return Ok(());
    }

    Err(
        "GSwitch found credentials changed outside the pending switch and will not overwrite them"
            .to_string(),
    )
}

pub fn remove_saved_account(state: &AppState, id: &str) -> Result<(), String> {
    let operation = state.acquire_operation()?;
    ensure_ready_for_credential_operation(state, &operation)?;
    runtime::ensure_no_external_codex(&[])?;
    let target = state.account_by_id_under_operation(&operation, id)?;
    let (_, target_identity) = stored_identity(&target)?;
    let codex_home = codex::codex_home()?;
    if codex::credential_store_mode(&codex_home)? == CredentialStoreMode::File {
        if let Some(current) = codex::read_optional_auth_document(&codex_home)? {
            if credential_identity(&current)?.1 == target_identity {
                return Err("The active Codex account cannot be removed".to_string());
            }
        }
    }
    state.remove_under_operation(&operation, id)
}

fn ensure_ready_for_credential_operation(
    state: &AppState,
    operation: &OperationGuard<'_>,
) -> Result<(), String> {
    if state.pending_switch_under_operation(operation)?.is_some() {
        return Err("GSwitch must recover a previous switch before starting another".to_string());
    }
    Ok(())
}

fn require_file_store(codex_home: &std::path::Path) -> Result<(), String> {
    match codex::credential_store_mode(codex_home)? {
        CredentialStoreMode::File => Ok(()),
        CredentialStoreMode::Keyring | CredentialStoreMode::Auto | CredentialStoreMode::Ephemeral => Err(
            "Enable account switching with a file-backed Codex credential before switching accounts"
                .to_string(),
        ),
        CredentialStoreMode::Unknown => Err("Codex credential storage mode is not supported by GSwitch".to_string()),
    }
}

/// A user config file alone cannot prove that enterprise or MDM policy did
/// not override the credential backend. Consult the supported App Server view
/// before any operation reads, saves, or replaces live credentials.
fn start_effective_file_store(codex_home: &std::path::Path) -> Result<AppServer, String> {
    require_file_store(codex_home)?;
    let mut server = AppServer::start(codex_home)?;
    runtime::ensure_no_external_codex(&[server.pid()])?;
    let config = server.config_read(1)?;
    reject_managed_store_origin(&config)?;
    if config_store_value(&config) != Some("file") {
        return Err(
            "Codex did not confirm file-backed credential storage for switching".to_string(),
        );
    }
    Ok(server)
}

fn validate_target_in_isolation(
    state: &AppState,
    operation: &OperationGuard<'_>,
    target: &StoredAccount,
    target_kind: &AccountKind,
    target_identity: &AccountIdentity,
) -> Result<Value, String> {
    let mut temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
    temporary.write_auth(&target.credential)?;
    let mut server = AppServer::start(&temporary.path)?;
    let metadata = account_metadata(&server.account_read(1, true)?)?;
    ensure_metadata_kind(&metadata, target_kind)?;
    let credential = temporary.read_auth()?;
    if document_kind(&credential)? != *target_kind
        || derive_identity(target_kind, &credential)? != *target_identity
    {
        return Err("Codex did not confirm the selected account identity".to_string());
    }
    if state
        .update_credential_under_operation(operation, &target.id, credential.clone())
        .is_err()
    {
        if state
            .record_pending_credential(operation, &credential)
            .is_err()
        {
            temporary.retain_for_recovery();
        }
        return Err(
            "GSwitch could not save validated credentials. A protected recovery copy was retained."
                .to_string(),
        );
    }
    Ok(credential)
}

fn confirm_already_active(
    state: &AppState,
    operation: &OperationGuard<'_>,
    codex_home: &std::path::Path,
    target: &StoredAccount,
    target_kind: &AccountKind,
    target_identity: &AccountIdentity,
) -> Result<SwitchOutcome, String> {
    let mut server = AppServer::start(codex_home)?;
    runtime::ensure_no_external_codex(&[server.pid()])?;
    let metadata = account_metadata(&server.account_read(1, false)?)?;
    ensure_metadata_kind(&metadata, target_kind)?;
    let refreshed = codex::read_auth_document(codex_home)?;
    if document_kind(&refreshed)? != *target_kind
        || derive_identity(target_kind, &refreshed)? != *target_identity
    {
        return Err("Codex did not confirm the active account identity".to_string());
    }
    update_credential_or_record(state, operation, &target.id, refreshed)?;
    let account = state.complete_switch_under_operation(operation, &target.id)?;
    Ok(SwitchOutcome { account })
}

fn verify_written_target(
    state: &AppState,
    operation: &OperationGuard<'_>,
    codex_home: &std::path::Path,
    target: &StoredAccount,
    target_kind: &AccountKind,
    target_identity: &AccountIdentity,
) -> Result<AccountView, String> {
    let mut server = AppServer::start(codex_home)?;
    runtime::ensure_no_external_codex(&[server.pid()])?;
    let metadata = account_metadata(&server.account_read(1, false)?)?;
    ensure_metadata_kind(&metadata, target_kind)?;
    let verified = codex::read_auth_document(codex_home)?;
    if document_kind(&verified)? != *target_kind
        || derive_identity(target_kind, &verified)? != *target_identity
    {
        return Err("Codex did not confirm the selected account identity".to_string());
    }
    if let Err(error) = update_credential_or_record(state, operation, &target.id, verified) {
        return Err(format!(
            "{error}; GSwitch will recover the verified switch on the next attempt"
        ));
    }
    state.mark_switch_verified_under_operation(operation)?;
    state.complete_switch_under_operation(operation, &target.id)
}

fn restore_previous_if_unchanged(
    state: &AppState,
    operation: &OperationGuard<'_>,
    codex_home: &std::path::Path,
    written_credential: &Value,
    previous_auth: Option<Value>,
) -> Result<(), String> {
    let Some(previous_auth) = previous_auth else {
        return Err("GSwitch left the verified credential in place for recovery because there was no prior file credential".to_string());
    };
    runtime::ensure_no_external_codex(&[])?;
    let current = codex::read_optional_auth_document(codex_home)?;
    if current.as_ref().map(document_fingerprint).transpose()?
        != Some(document_fingerprint(written_credential)?)
    {
        return Err(
            "GSwitch will not restore because Codex credentials changed after the switch attempt"
                .to_string(),
        );
    }
    codex::write_auth_document(codex_home, &previous_auth)?;
    let previous_active_id = state
        .pending_switch_under_operation(operation)?
        .map(|pending| pending.previous_active_id)
        .ok_or_else(|| "No GSwitch switch transaction is pending".to_string())?;
    state.clear_pending_switch_under_operation(operation, previous_active_id)?;
    Ok(())
}

fn update_credential_or_record(
    state: &AppState,
    operation: &OperationGuard<'_>,
    id: &str,
    credential: Value,
) -> Result<(), String> {
    if state
        .update_credential_under_operation(operation, id, credential.clone())
        .is_ok()
    {
        return Ok(());
    }
    if state
        .record_pending_credential(operation, &credential)
        .is_ok()
    {
        return Err(
            "GSwitch could not save refreshed credentials. A protected recovery copy was retained."
                .to_string(),
        );
    }
    Err("GSwitch could not save refreshed credentials or retain a recovery copy".to_string())
}

fn stored_identity(account: &StoredAccount) -> Result<(AccountKind, AccountIdentity), String> {
    let identity = account.identity.clone().ok_or_else(|| {
        "The saved account needs to be added again before it can be switched".to_string()
    })?;
    let derived_kind = document_kind(&account.credential)?;
    let derived_identity = derive_identity(&derived_kind, &account.credential)?;
    if account.kind != derived_kind || identity != derived_identity {
        return Err(
            "The saved account credentials do not match their recorded identity".to_string(),
        );
    }
    Ok((account.kind.clone(), identity))
}

fn credential_identity(credential: &Value) -> Result<(AccountKind, AccountIdentity), String> {
    let kind = document_kind(credential)?;
    let identity = derive_identity(&kind, credential)?;
    Ok((kind, identity))
}

fn ensure_metadata_kind(metadata: &AccountMetadata, expected: &AccountKind) -> Result<(), String> {
    if &metadata.kind == expected {
        Ok(())
    } else {
        Err("Codex verified a different account type".to_string())
    }
}

fn account_kind_for_identity(identity: &AccountIdentity) -> AccountKind {
    match identity {
        AccountIdentity::ChatGpt { .. } => AccountKind::ChatGpt,
        AccountIdentity::ApiKey { .. } => AccountKind::ApiKey,
    }
}

fn origin_version(config: &Value) -> Option<&str> {
    config
        .pointer("/origins/cli_auth_credentials_store/version")
        .or_else(|| config.pointer("/origins/cliAuthCredentialsStore/version"))
        .and_then(Value::as_str)
}

fn origin_type(config: &Value) -> Option<&str> {
    config
        .pointer("/origins/cli_auth_credentials_store/name/type")
        .or_else(|| config.pointer("/origins/cliAuthCredentialsStore/name/type"))
        .or_else(|| config.pointer("/origins/cli_auth_credentials_store/name"))
        .or_else(|| config.pointer("/origins/cliAuthCredentialsStore/name"))
        .and_then(Value::as_str)
}

fn reject_managed_store_origin(config: &Value) -> Result<(), String> {
    if matches!(
        origin_type(config),
        Some(
            "mdm"
                | "enterpriseManaged"
                | "enterprise_managed"
                | "legacyManaged"
                | "legacy_managed"
                | "legacyManagedToml"
                | "legacy_managed_toml"
        )
    ) {
        return Err("Codex administrator policy controls credential storage".to_string());
    }
    Ok(())
}

fn config_store_value(config: &Value) -> Option<&str> {
    config
        .pointer("/config/cli_auth_credentials_store")
        .or_else(|| config.pointer("/config/cliAuthCredentialsStore"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_managed_credential_store_origin() {
        let config = json!({
            "origins": {"cli_auth_credentials_store": {"name": {"type": "enterprise_managed"}, "version": "3"}}
        });
        assert_eq!(
            reject_managed_store_origin(&config).expect_err("managed"),
            "Codex administrator policy controls credential storage"
        );
    }

    #[test]
    fn reads_snake_or_camel_case_protocol_fields() {
        let snake = json!({"config": {"cli_auth_credentials_store": "file"}});
        let camel = json!({"config": {"cliAuthCredentialsStore": "file"}});
        assert_eq!(config_store_value(&snake), Some("file"));
        assert_eq!(config_store_value(&camel), Some("file"));
    }
}
