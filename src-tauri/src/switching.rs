use serde_json::{json, Value};

use crate::{
    accounts::{AppState, OperationAcquireFailure, OperationGuard},
    app_server::{account_metadata, AccountMetadata, AppServer, TempCodexHome},
    chatgpt::{self, ChatGptClient, RequestFailureKind},
    codex,
    identity::{derive_identity, document_fingerprint, document_kind},
    runtime,
    types::{
        AccountIdentity, AccountKind, AccountView, CredentialStoreMode, LiveAccountStatus,
        LiveAccountView, PendingSwitch, PendingSwitchStage, StoredAccount, SwitchFailure,
        SwitchFailureCode, SwitchOutcome,
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
    let codex_home = codex::codex_home()?;
    save_current_account_at(state, &codex_home, ChatGptClient::new)
}

fn save_current_account_at<F>(
    state: &AppState,
    codex_home: &std::path::Path,
    make_chatgpt_client: F,
) -> Result<AccountView, String>
where
    F: FnOnce() -> Result<ChatGptClient, String>,
{
    let operation = state.acquire_operation()?;
    ensure_ready_for_credential_operation(state, &operation)?;

    // This action never writes the live profile. In particular, a running
    // Codex process must not turn a read/save operation into token refresh.
    require_file_store(codex_home)?;
    let original_credential = codex::read_optional_auth_document(codex_home)?
        .ok_or_else(|| "Codex is not signed in with a file-backed credential".to_string())?;
    let kind = document_kind(&original_credential)?;
    let identity = derive_identity(&kind, &original_credential)?;
    let (metadata, credential) = match kind {
        AccountKind::ChatGpt => save_current_chatgpt_snapshot(
            codex_home,
            original_credential,
            &identity,
            &make_chatgpt_client()?,
        )?,
        AccountKind::ApiKey => {
            let temporary = TempCodexHome::create(&state.isolated_profile_root()?)?;
            temporary.write_auth(&original_credential)?;
            let mut server = AppServer::start(&temporary.path)?;
            let metadata = account_metadata(&server.account_read(2, false)?)?;
            ensure_metadata_kind(&metadata, &kind)?;
            let credential = temporary.read_auth()?;
            ensure_credential_identity(
                &credential,
                &kind,
                &identity,
                "Codex did not confirm the current account identity",
            )?;
            let current_credential = codex::read_auth_document(codex_home)?;
            ensure_credential_identity(
                &current_credential,
                &kind,
                &identity,
                "Codex credentials changed while saving the current account",
            )?;
            (metadata, credential)
        }
    };
    let default_label = metadata
        .email
        .clone()
        .or_else(|| metadata.workspace_name.clone())
        .unwrap_or_else(|| match kind {
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
            workspace_name: metadata.workspace_name,
            account_structure: metadata.account_structure,
            identity,
            credential,
        },
    );
    if result.is_err() {
        return if state
            .record_pending_credential(&operation, &recovery_credential)
            .is_ok()
        {
            Err("GSwitch could not save the current account. A protected recovery copy was retained.".to_string())
        } else {
            Err("GSwitch could not save the current account or write protected recovery. No plaintext recovery copy was retained.".to_string())
        };
    }
    result
}

fn save_current_chatgpt_snapshot(
    codex_home: &std::path::Path,
    original: Value,
    identity: &AccountIdentity,
    client: &ChatGptClient,
) -> Result<(AccountMetadata, Value), String> {
    let mut snapshot = original;
    for attempt in 0..=1 {
        let response = client
            .account_check(&snapshot)
            .map_err(|_| "ChatGPT could not validate the current account snapshot".to_string())?;
        chatgpt::validate_response_identity(&snapshot, identity, &response)?;
        let projection = chatgpt::normalize_account_metadata(&snapshot, identity, &response)?;

        // Codex may rotate its own credential while the provider request is in
        // flight. Never save the older copy or validate a newer copy by proxy.
        let live = codex::read_auth_document(codex_home)?;
        ensure_credential_identity(
            &live,
            &AccountKind::ChatGpt,
            identity,
            "Codex credentials changed while saving the current account",
        )?;
        if live == snapshot {
            return Ok((
                AccountMetadata {
                    kind: AccountKind::ChatGpt,
                    email: projection.email,
                    plan_type: projection.plan_type,
                    workspace_name: projection.workspace_name,
                    account_structure: projection.account_structure,
                },
                live,
            ));
        }
        if attempt == 1 {
            return Err("Codex credentials changed again while saving the current account".into());
        }
        snapshot = live;
    }
    unreachable!("the bounded snapshot check always returns")
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

pub fn switch_account(state: &AppState, target_id: &str) -> Result<SwitchOutcome, SwitchFailure> {
    let operation = state.acquire_operation_for_switch().map_err(|error| {
        let code = match error {
            OperationAcquireFailure::Busy => SwitchFailureCode::OperationBusy,
            OperationAcquireFailure::Failed(_) => SwitchFailureCode::LocalVerificationFailed,
        };
        switch_failure(code)
    })?;
    if state
        .pending_switch_under_operation(&operation)
        .map_err(|_| switch_failure(SwitchFailureCode::RecoveryRequired))?
        .is_some()
    {
        return Err(switch_failure(SwitchFailureCode::RecoveryRequired));
    }

    // This cheap guard intentionally precedes configuration and provider work.
    runtime::ensure_no_external_codex(&[])
        .map_err(|_| switch_failure(SwitchFailureCode::CodexOpen))?;
    let codex_home = codex::codex_home()
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?;
    check_effective_file_store(state, &codex_home).map_err(switch_failure)?;
    let target = state
        .account_by_id_under_operation(&operation, target_id)
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?;
    let (target_kind, target_identity) = stored_identity(&target)
        .map_err(|_| switch_failure(SwitchFailureCode::AccountNeedsSignIn))?;

    let previous_auth = codex::read_optional_auth_document(&codex_home)
        .map_err(|_| switch_failure(SwitchFailureCode::CurrentCredentialUnreadable))?;
    let previous_fingerprint = previous_auth
        .as_ref()
        .map(document_fingerprint)
        .transpose()
        .map_err(|_| switch_failure(SwitchFailureCode::CurrentCredentialUnreadable))?;
    if let Some(current) = previous_auth.as_ref() {
        let (_, current_identity) = credential_identity(current)
            .map_err(|_| switch_failure(SwitchFailureCode::CurrentCredentialUnreadable))?;
        let current_saved = state
            .account_by_identity_under_operation(&operation, &current_identity)
            .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?
            .ok_or_else(|| switch_failure(SwitchFailureCode::CurrentAccountNotSaved))?;
        if current_identity == target_identity {
            validate_target_snapshot(state, &operation, &target, &target_kind, &target_identity)?;
            return confirm_already_active(state, &operation, &target);
        }
        update_credential_or_record(state, &operation, &current_saved.id, current.clone())
            .map_err(|_| switch_failure(SwitchFailureCode::RecoveryRequired))?;
    }

    let validated_credential =
        validate_target_snapshot(state, &operation, &target, &target_kind, &target_identity)?;

    let pending = PendingSwitch {
        target_id: target.id.clone(),
        target_identity: target_identity.clone(),
        previous_active_id: state
            .active_account_id_under_operation(&operation)
            .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?,
        secret_ref: None,
        secret_generation: 0,
        previous_auth: previous_auth.clone(),
        stage: PendingSwitchStage::Prepared,
    };
    state
        .prepare_switch_under_operation(&operation, pending)
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?;

    // A process could start or a user could edit auth.json after validation.
    // Both checks must pass before this is allowed to replace the live file.
    runtime::ensure_no_external_codex(&[])
        .map_err(|_| switch_failure(SwitchFailureCode::CodexOpen))?;
    let observed = codex::read_optional_auth_document(&codex_home)
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?;
    if observed
        .as_ref()
        .map(document_fingerprint)
        .transpose()
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?
        != previous_fingerprint
    {
        return Err(switch_failure(SwitchFailureCode::CredentialsChanged));
    }
    codex::write_auth_document(&codex_home, &validated_credential)
        .map_err(|_| switch_failure(SwitchFailureCode::RecoveryRequired))?;

    match verify_written_target(state, &operation, &codex_home, &target, &target_identity) {
        Ok(account) => Ok(SwitchOutcome { account }),
        Err(_) => match restore_previous_if_unchanged(
            state,
            &operation,
            &codex_home,
            &validated_credential,
            previous_auth,
        ) {
            Ok(()) => Err(switch_failure(
                SwitchFailureCode::PostWriteVerificationFailed,
            )),
            Err(_) => Err(switch_failure(SwitchFailureCode::RecoveryRequired)),
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
    check_effective_file_store(state, &codex_home).map_err(|_| {
        "Unable to confirm file-backed Codex configuration for recovery".to_string()
    })?;
    let current = codex::read_optional_auth_document(&codex_home)?;

    if let Some(current) = current {
        let (kind, identity) = credential_identity(&current)?;
        if kind == account_kind_for_identity(&pending.target_identity)
            && identity == pending.target_identity
        {
            let target = state.account_by_id_under_operation(&operation, &pending.target_id)?;
            let (_, target_identity) = stored_identity(&target)?;
            return verify_written_target(
                state,
                &operation,
                &codex_home,
                &target,
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
/// Inspect effective policy in a clean, GSwitch-owned Codex profile. The
/// user profile is checked locally for an explicit file store, while the
/// isolated App Server reveals machine policy without initializing against
/// the live auth/config directory that may be held by another application.
fn check_effective_file_store(
    state: &AppState,
    codex_home: &std::path::Path,
) -> Result<(), SwitchFailureCode> {
    require_file_store(codex_home).map_err(|_| SwitchFailureCode::FileStoreRequired)?;
    let profile = TempCodexHome::create(
        &state
            .isolated_profile_root()
            .map_err(|_| SwitchFailureCode::CodexConfigUnavailable)?,
    )
    .map_err(|_| SwitchFailureCode::CodexConfigUnavailable)?;
    let mut server =
        AppServer::start(&profile.path).map_err(|_| SwitchFailureCode::CodexConfigUnavailable)?;
    runtime::ensure_no_external_codex(&[server.pid()]).map_err(|_| SwitchFailureCode::CodexOpen)?;
    let config = server
        .config_read(1)
        .map_err(|_| SwitchFailureCode::CodexConfigUnavailable)?;
    reject_managed_store_origin(&config).map_err(|_| SwitchFailureCode::FileStoreRequired)?;
    if config_store_value(&config) != Some("file") {
        return Err(SwitchFailureCode::FileStoreRequired);
    }
    drop(server);
    profile
        .cleanup()
        .map_err(|_| SwitchFailureCode::CodexConfigUnavailable)?;
    Ok(())
}

fn validate_target_snapshot(
    state: &AppState,
    operation: &OperationGuard<'_>,
    target: &StoredAccount,
    target_kind: &AccountKind,
    target_identity: &AccountIdentity,
) -> Result<Value, SwitchFailure> {
    if target_kind == &AccountKind::ApiKey {
        return Ok(target.credential.clone());
    }

    let client = ChatGptClient::new()
        .map_err(|_| switch_failure(SwitchFailureCode::TargetCheckUnavailable))?;
    validate_chatgpt_snapshot_with(
        state,
        operation,
        target,
        target_identity,
        &client,
        || {
            runtime::ensure_no_external_codex(&[])
                .map_err(|_| switch_failure(SwitchFailureCode::CodexOpen))
        },
        || managed_refresh_target(state, target),
    )
}

struct ManagedRefresh {
    credential: Value,
    metadata: AccountMetadata,
}

fn validate_chatgpt_snapshot_with<P, F>(
    state: &AppState,
    operation: &OperationGuard<'_>,
    target: &StoredAccount,
    target_identity: &AccountIdentity,
    client: &ChatGptClient,
    refresh_precondition: P,
    managed_refresh: F,
) -> Result<Value, SwitchFailure>
where
    P: FnOnce() -> Result<(), SwitchFailure>,
    F: FnOnce() -> Result<ManagedRefresh, String>,
{
    match client.account_check(&target.credential) {
        Ok(response) => {
            chatgpt::validate_response_identity(&target.credential, target_identity, &response)
                .map_err(|_| switch_failure(SwitchFailureCode::TargetWorkspaceMismatch))?;
            let projection =
                chatgpt::normalize_account_metadata(&target.credential, target_identity, &response)
                    .map_err(|_| switch_failure(SwitchFailureCode::TargetWorkspaceMismatch))?;
            let metadata = AccountMetadata {
                kind: AccountKind::ChatGpt,
                email: projection.email,
                plan_type: projection.plan_type,
                workspace_name: projection.workspace_name,
                account_structure: projection.account_structure,
            };
            persist_switch_validation(state, operation, target, &metadata, None)?;
            Ok(target.credential.clone())
        }
        Err(error) if error.kind == RequestFailureKind::Authentication => {
            refresh_precondition()?;
            let refreshed = managed_refresh()
                .map_err(|_| switch_failure(SwitchFailureCode::AccountNeedsSignIn))?;
            ensure_metadata_kind(&refreshed.metadata, &AccountKind::ChatGpt)
                .map_err(|_| switch_failure(SwitchFailureCode::AccountNeedsSignIn))?;
            ensure_credential_identity(
                &refreshed.credential,
                &AccountKind::ChatGpt,
                target_identity,
                "Codex refreshed a different account",
            )
            .map_err(|_| switch_failure(SwitchFailureCode::AccountNeedsSignIn))?;
            persist_switch_validation(
                state,
                operation,
                target,
                &refreshed.metadata,
                Some(refreshed.credential.clone()),
            )?;
            Ok(refreshed.credential)
        }
        Err(_) => Err(switch_failure(SwitchFailureCode::TargetCheckUnavailable)),
    }
}

fn managed_refresh_target(
    state: &AppState,
    target: &StoredAccount,
) -> Result<ManagedRefresh, String> {
    let profile = TempCodexHome::create(&state.isolated_profile_root()?)?;
    profile.write_auth(&target.credential)?;
    let mut server = AppServer::start(&profile.path)?;
    let metadata = account_metadata(&server.account_read(1, true)?)?;
    let credential = profile.read_auth()?;
    Ok(ManagedRefresh {
        credential,
        metadata,
    })
}

fn persist_switch_validation(
    state: &AppState,
    operation: &OperationGuard<'_>,
    target: &StoredAccount,
    metadata: &AccountMetadata,
    credential: Option<Value>,
) -> Result<(), SwitchFailure> {
    if state
        .update_switch_validation_under_operation(
            operation,
            &target.id,
            metadata,
            credential.clone(),
        )
        .is_ok()
    {
        return Ok(());
    }

    let Some(credential) = credential else {
        return Err(switch_failure(SwitchFailureCode::LocalVerificationFailed));
    };
    match state.record_pending_credential(operation, &credential) {
        Ok(()) => Err(switch_failure(SwitchFailureCode::RecoveryRequired)),
        Err(_) => Err(switch_failure(SwitchFailureCode::AccountNeedsSignIn)),
    }
}

fn confirm_already_active(
    state: &AppState,
    operation: &OperationGuard<'_>,
    target: &StoredAccount,
) -> Result<SwitchOutcome, SwitchFailure> {
    let account = state
        .complete_switch_under_operation(operation, &target.id)
        .map_err(|_| switch_failure(SwitchFailureCode::LocalVerificationFailed))?;
    Ok(SwitchOutcome { account })
}

fn verify_written_target(
    state: &AppState,
    operation: &OperationGuard<'_>,
    codex_home: &std::path::Path,
    target: &StoredAccount,
    target_identity: &AccountIdentity,
) -> Result<AccountView, String> {
    let verified = codex::read_auth_document(codex_home)?;
    let verified_kind = document_kind(&verified)?;
    if derive_identity(&verified_kind, &verified)? != *target_identity {
        return Err("Codex did not confirm the selected account identity".to_string());
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
    restore_previous_if_unchanged_with(
        state,
        operation,
        codex_home,
        written_credential,
        previous_auth,
        || runtime::ensure_no_external_codex(&[]),
    )
}

fn restore_previous_if_unchanged_with<F>(
    state: &AppState,
    operation: &OperationGuard<'_>,
    codex_home: &std::path::Path,
    written_credential: &Value,
    previous_auth: Option<Value>,
    ensure_exclusive: F,
) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    let Some(previous_auth) = previous_auth else {
        return Err("GSwitch left the verified credential in place for recovery because there was no prior file credential".to_string());
    };
    ensure_exclusive()?;
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
    Err("GSwitch could not save refreshed credentials or write protected recovery. No plaintext recovery copy was retained.".to_string())
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

fn ensure_credential_identity(
    credential: &Value,
    expected_kind: &AccountKind,
    expected_identity: &AccountIdentity,
    message: &str,
) -> Result<(), String> {
    if document_kind(credential)? != *expected_kind
        || derive_identity(expected_kind, credential)? != *expected_identity
    {
        return Err(message.to_string());
    }
    Ok(())
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

fn switch_failure(code: SwitchFailureCode) -> SwitchFailure {
    SwitchFailure::new(code)
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
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    fn credential(user: &str, workspace: &str, access_token: &str) -> Value {
        let claims = json!({
            "https://api.openai.com/auth": {
                "chatgpt_user_id": user,
                "chatgpt_account_id": workspace
            }
        });
        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"))
        );
        json!({"tokens": {"id_token": id_token, "access_token": access_token}})
    }

    fn state_with_chatgpt_target() -> (AppState, StoredAccount, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("gswitch-switching-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test directory");
        let state = AppState::new(root.join("accounts.json")).expect("state");
        let saved = {
            let operation = state.acquire_operation().expect("operation");
            state
                .upsert_under_operation(
                    &operation,
                    crate::accounts::AccountDraft {
                        label: None,
                        default_label: "person@example.com".into(),
                        kind: AccountKind::ChatGpt,
                        email: Some("old@example.com".into()),
                        plan_type: None,
                        workspace_name: Some("Personal".into()),
                        account_structure: Some("workspace".into()),
                        identity: AccountIdentity::ChatGpt {
                            user_id: "user".into(),
                            workspace_id: Some("workspace".into()),
                        },
                        credential: credential("user", "workspace", "access-token"),
                    },
                )
                .expect("save")
        };
        let operation = state.acquire_operation().expect("operation");
        let target = state
            .account_by_id_under_operation(&operation, &saved.id)
            .expect("target");
        drop(operation);
        (state, target, root)
    }

    #[test]
    fn switch_reports_a_busy_operation_lock_as_actionable() {
        let (state, target, root) = state_with_chatgpt_target();
        let operation = state.acquire_operation().expect("operation lock");

        let error = switch_account(&state, &target.id).expect_err("busy operation");
        assert_eq!(error.code, SwitchFailureCode::OperationBusy);

        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    fn account_check_server(
        status: u16,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0; 4096];
            let size = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
            request
        });
        (format!("http://{address}"), handle)
    }

    fn current_account_state(
        credential: &Value,
    ) -> (AppState, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("gswitch-current-{}", uuid::Uuid::new_v4()));
        let codex_home = root.join("codex-home");
        fs::create_dir_all(&codex_home).expect("Codex home");
        fs::write(
            codex_home.join("config.toml"),
            "cli_auth_credentials_store = \"file\"\n",
        )
        .expect("file-backed configuration");
        codex::write_auth_document(&codex_home, credential).expect("live credential");
        let state = AppState::new(root.join("accounts.json")).expect("state");
        (state, codex_home, root)
    }

    fn current_account_check_sequence(
        codex_home: std::path::PathBuf,
        steps: Vec<(u16, &'static str, Option<Value>)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body, next_live) in steps {
                let (mut stream, _) = listener.accept().expect("request");
                let mut buffer = [0; 4096];
                let size = stream.read(&mut buffer).expect("read request");
                requests.push(String::from_utf8_lossy(&buffer[..size]).to_string());
                if let Some(next_live) = next_live {
                    codex::write_auth_document(&codex_home, &next_live)
                        .expect("Codex-owned live rotation");
                }
                let reason = if status == 200 { "OK" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

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

    #[test]
    fn saving_current_account_accepts_a_rotated_token_for_the_same_identity() {
        let original = credential("user", "workspace", "old-token");
        let rotated = credential("user", "workspace", "new-token");
        let identity = derive_identity(&AccountKind::ChatGpt, &original).expect("identity");

        ensure_credential_identity(&rotated, &AccountKind::ChatGpt, &identity, "changed")
            .expect("same account");
        assert_ne!(original, rotated);
    }

    #[test]
    fn saving_current_account_rejects_a_live_identity_change() {
        let original = credential("user", "workspace", "old-token");
        let changed = credential("other-user", "workspace", "new-token");
        let identity = derive_identity(&AccountKind::ChatGpt, &original).expect("identity");

        assert_eq!(
            ensure_credential_identity(&changed, &AccountKind::ChatGpt, &identity, "changed")
                .expect_err("different account"),
            "changed"
        );
    }

    #[test]
    fn current_chatgpt_snapshot_saves_exact_live_document_without_managed_refresh() {
        let mut original = credential("user", "workspace", "access-token");
        original["tokens"]["refresh_token"] = json!("codex-owned-refresh");
        original["future_field"] = json!({"keep": [1, "unknown"]});
        let (state, codex_home, root) = current_account_state(&original);
        let live_before = fs::read(codex::auth_path(&codex_home)).expect("live bytes");
        let (base_url, server) = account_check_server(
            200,
            r#"{"accounts":[{"id":"workspace","name":"Personal","structure":"personal","plan_type":"plus"}]}"#,
        );
        let account = save_current_account_at(&state, &codex_home, || {
            ChatGptClient::with_base_url(&base_url)
        })
        .expect("save current ChatGPT account");
        let request = server.join().expect("account check");
        assert!(request.starts_with("GET /wham/accounts/check HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));
        assert_eq!(
            fs::read(codex::auth_path(&codex_home)).expect("live bytes"),
            live_before
        );
        assert_eq!(
            state.account_by_id(&account.id).expect("saved").credential,
            original
        );
        assert_eq!(account.workspace_name.as_deref(), Some("Personal"));
        assert_eq!(account.plan_type.as_deref(), Some("plus"));
        assert!(!state
            .isolated_profile_root()
            .expect("profile root")
            .exists());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn current_chatgpt_snapshot_revalidates_one_newer_same_identity_document() {
        let original = credential("user", "workspace", "old-access");
        let mut newer = credential("user", "workspace", "new-access");
        newer["tokens"]["refresh_token"] = json!("new-codex-owned-refresh");
        newer["future_field"] = json!({"survives": true});
        let (state, codex_home, root) = current_account_state(&original);
        let (base_url, server) = current_account_check_sequence(
            codex_home.clone(),
            vec![
                (
                    200,
                    r#"{"accounts":[{"id":"workspace","name":"Old"}]}"#,
                    Some(newer.clone()),
                ),
                (
                    200,
                    r#"{"accounts":[{"id":"workspace","name":"New"}]}"#,
                    None,
                ),
            ],
        );
        let account = save_current_account_at(&state, &codex_home, || {
            ChatGptClient::with_base_url(&base_url)
        })
        .expect("save newest live credential");
        let requests = server.join().expect("account checks");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("authorization: Bearer old-access"));
        assert!(requests[1].contains("authorization: Bearer new-access"));
        assert_eq!(
            state.account_by_id(&account.id).expect("saved").credential,
            newer
        );
        assert_eq!(account.workspace_name.as_deref(), Some("New"));
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn current_chatgpt_snapshot_rejects_a_live_identity_change() {
        let original = credential("user", "workspace", "old-access");
        let changed = credential("other-user", "workspace", "different-access");
        let (state, codex_home, root) = current_account_state(&original);
        let (base_url, server) = current_account_check_sequence(
            codex_home.clone(),
            vec![(
                200,
                r#"{"accounts":[{"id":"workspace"}]}"#,
                Some(changed.clone()),
            )],
        );
        let error = save_current_account_at(&state, &codex_home, || {
            ChatGptClient::with_base_url(&base_url)
        })
        .expect_err("identity changed");
        assert_eq!(
            error,
            "Codex credentials changed while saving the current account"
        );
        assert_eq!(server.join().expect("account check").len(), 1);
        assert!(state.list().expect("accounts").is_empty());
        assert_eq!(
            codex::read_auth_document(&codex_home).expect("live"),
            changed
        );
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn current_chatgpt_snapshot_rejects_a_workspace_mismatch_without_saving() {
        let original = credential("user", "workspace", "access-token");
        let (state, codex_home, root) = current_account_state(&original);
        let (base_url, server) = account_check_server(
            200,
            r#"{"accounts":[{"id":"different-workspace","name":"Wrong"}]}"#,
        );
        let error = save_current_account_at(&state, &codex_home, || {
            ChatGptClient::with_base_url(&base_url)
        })
        .expect_err("workspace mismatch");
        assert_eq!(
            error,
            "ChatGPT did not return the expected account workspace"
        );
        server.join().expect("account check");
        assert!(state.list().expect("accounts").is_empty());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn current_chatgpt_auth_failure_never_starts_managed_refresh() {
        let original = credential("user", "workspace", "access-token");
        let (state, codex_home, root) = current_account_state(&original);
        let (base_url, server) = account_check_server(401, r#"{}"#);
        let error = save_current_account_at(&state, &codex_home, || {
            ChatGptClient::with_base_url(&base_url)
        })
        .expect_err("read-only failure");
        assert_eq!(
            error,
            "ChatGPT could not validate the current account snapshot"
        );
        assert_eq!(
            server
                .join()
                .expect("account check")
                .matches("GET ")
                .count(),
            1
        );
        assert!(state.list().expect("accounts").is_empty());
        assert!(!state
            .isolated_profile_root()
            .expect("profile root")
            .exists());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn valid_snapshot_switch_validation_never_starts_managed_refresh() {
        let (state, target, root) = state_with_chatgpt_target();
        let (base_url, server) = account_check_server(
            200,
            r#"{"accounts":[{"id":"workspace","name":"Updated","structure":"workspace","plan_type":"plus"}]}"#,
        );
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let identity = target.identity.clone().expect("identity");
        let operation = state.acquire_operation().expect("operation");

        let validated = validate_chatgpt_snapshot_with(
            &state,
            &operation,
            &target,
            &identity,
            &client,
            || Ok(()),
            || panic!("valid snapshots must not start managed refresh"),
        )
        .expect("validated");

        assert_eq!(validated, target.credential);
        let request = server.join().expect("server");
        assert!(request.starts_with("GET /wham/accounts/check"));
        let saved = state
            .account_by_id_under_operation(&operation, &target.id)
            .expect("saved");
        assert_eq!(saved.workspace_name.as_deref(), Some("Updated"));
        assert_eq!(saved.credential, target.credential);
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn authentication_failure_allows_exactly_one_identity_checked_refresh() {
        let (state, target, root) = state_with_chatgpt_target();
        let (base_url, server) = account_check_server(401, r#"{}"#);
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let identity = target.identity.clone().expect("identity");
        let calls = Arc::new(Mutex::new(0_u8));
        let counted = Arc::clone(&calls);
        let refreshed_credential = credential("user", "workspace", "refreshed-token");
        let operation = state.acquire_operation().expect("operation");

        let validated = validate_chatgpt_snapshot_with(
            &state,
            &operation,
            &target,
            &identity,
            &client,
            || Ok(()),
            || {
                *counted.lock().expect("counter") += 1;
                Ok(ManagedRefresh {
                    credential: refreshed_credential.clone(),
                    metadata: AccountMetadata {
                        kind: AccountKind::ChatGpt,
                        email: Some("person@example.com".into()),
                        plan_type: Some("plus".into()),
                        workspace_name: Some("Personal".into()),
                        account_structure: Some("workspace".into()),
                    },
                })
            },
        )
        .expect("refreshed");

        assert_eq!(*calls.lock().expect("counter"), 1);
        assert_eq!(validated, refreshed_credential);
        let saved = state
            .account_by_id_under_operation(&operation, &target.id)
            .expect("saved");
        assert_eq!(saved.credential, refreshed_credential);
        server.join().expect("server");
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_protected_recovery_reports_sign_in_instead_of_claiming_recovery_exists() {
        let (state, target, root) = state_with_chatgpt_target();
        let operation = state.acquire_operation().expect("operation");
        let store_path = root.join("accounts.json");
        let vault_path = root.join("credentials.hold");

        fs::remove_file(&store_path).expect("remove metadata file");
        fs::create_dir(&store_path).expect("block metadata commit");
        fs::remove_file(&vault_path).expect("remove vault snapshot");
        fs::create_dir(&vault_path).expect("block protected recovery");

        let metadata = AccountMetadata {
            kind: AccountKind::ChatGpt,
            email: Some("person@example.com".into()),
            plan_type: Some("plus".into()),
            workspace_name: Some("Updated workspace".into()),
            account_structure: Some("workspace".into()),
        };
        let error = persist_switch_validation(
            &state,
            &operation,
            &target,
            &metadata,
            Some(credential("user", "workspace", "rotated-token")),
        )
        .expect_err("metadata and protected recovery writes both fail");

        assert_eq!(error.code, SwitchFailureCode::AccountNeedsSignIn);
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn non_authentication_provider_failures_never_refresh() {
        for (status, body) in [(429, r#"{}"#), (500, r#"{}"#), (200, r#"not-json"#)] {
            let (state, target, root) = state_with_chatgpt_target();
            let (base_url, server) = account_check_server(status, body);
            let client = ChatGptClient::with_base_url(&base_url).expect("client");
            let identity = target.identity.clone().expect("identity");
            let operation = state.acquire_operation().expect("operation");

            let error = validate_chatgpt_snapshot_with(
                &state,
                &operation,
                &target,
                &identity,
                &client,
                || Ok(()),
                || panic!("non-authentication failures must not refresh"),
            )
            .expect_err("provider failure");

            assert_eq!(error.code, SwitchFailureCode::TargetCheckUnavailable);
            server.join().expect("server");
            drop(operation);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn target_workspace_mismatch_is_reported_before_any_live_write() {
        let (state, target, root) = state_with_chatgpt_target();
        let (base_url, server) = account_check_server(
            200,
            r#"{"accounts":[{"id":"different-workspace","name":"Other"}]}"#,
        );
        let client = ChatGptClient::with_base_url(&base_url).expect("client");
        let identity = target.identity.clone().expect("identity");
        let operation = state.acquire_operation().expect("operation");

        let error = validate_chatgpt_snapshot_with(
            &state,
            &operation,
            &target,
            &identity,
            &client,
            || Ok(()),
            || panic!("a workspace mismatch must not refresh"),
        )
        .expect_err("workspace mismatch");
        assert_eq!(error.code, SwitchFailureCode::TargetWorkspaceMismatch);
        assert_eq!(
            state
                .account_by_id_under_operation(&operation, &target.id)
                .expect("saved")
                .credential,
            target.credential
        );
        server.join().expect("server");
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn api_key_validation_is_local_only() {
        let root =
            std::env::temp_dir().join(format!("gswitch-switching-api-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("test directory");
        let state = AppState::new(root.join("accounts.json")).expect("state");
        let credential = json!({"OPENAI_API_KEY": "sk-test-key"});
        let identity = derive_identity(&AccountKind::ApiKey, &credential).expect("identity");
        let operation = state.acquire_operation().expect("operation");
        let account = state
            .upsert_under_operation(
                &operation,
                crate::accounts::AccountDraft {
                    label: None,
                    default_label: "API key".into(),
                    kind: AccountKind::ApiKey,
                    email: None,
                    plan_type: None,
                    workspace_name: None,
                    account_structure: None,
                    identity: identity.clone(),
                    credential: credential.clone(),
                },
            )
            .expect("save");
        let target = state
            .account_by_id_under_operation(&operation, &account.id)
            .expect("target");

        let validated =
            validate_target_snapshot(&state, &operation, &target, &AccountKind::ApiKey, &identity)
                .expect("local validation");
        assert_eq!(validated, credential);
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn written_target_is_confirmed_locally_and_committed() {
        let (state, target, root) = state_with_chatgpt_target();
        let identity = target.identity.clone().expect("identity");
        let codex_home = root.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_switch_under_operation(
                &operation,
                PendingSwitch {
                    target_id: target.id.clone(),
                    target_identity: identity.clone(),
                    previous_active_id: None,
                    secret_ref: None,
                    secret_generation: 0,
                    previous_auth: None,
                    stage: PendingSwitchStage::Prepared,
                },
            )
            .expect("pending");
        codex::write_auth_document(&codex_home, &target.credential).expect("write");

        let account = verify_written_target(&state, &operation, &codex_home, &target, &identity)
            .expect("verify");

        assert!(account.active);
        assert!(state
            .pending_switch_under_operation(&operation)
            .expect("pending")
            .is_none());
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_restores_only_the_exact_document_gswitch_wrote() {
        let (state, target, root) = state_with_chatgpt_target();
        let identity = target.identity.clone().expect("identity");
        let previous = credential("previous-user", "previous-workspace", "previous-token");
        let written = target.credential.clone();
        let codex_home = root.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_switch_under_operation(
                &operation,
                PendingSwitch {
                    target_id: target.id.clone(),
                    target_identity: identity,
                    previous_active_id: None,
                    secret_ref: None,
                    secret_generation: 0,
                    previous_auth: Some(previous.clone()),
                    stage: PendingSwitchStage::Prepared,
                },
            )
            .expect("pending");
        codex::write_auth_document(&codex_home, &written).expect("write");

        restore_previous_if_unchanged_with(
            &state,
            &operation,
            &codex_home,
            &written,
            Some(previous.clone()),
            || Ok(()),
        )
        .expect("restore");

        assert_eq!(
            codex::read_auth_document(&codex_home).expect("restored"),
            previous
        );
        assert!(state
            .pending_switch_under_operation(&operation)
            .expect("pending")
            .is_none());
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_preserves_an_external_change_and_pending_recovery() {
        let (state, target, root) = state_with_chatgpt_target();
        let identity = target.identity.clone().expect("identity");
        let previous = credential("previous-user", "previous-workspace", "previous-token");
        let written = target.credential.clone();
        let external = credential("external-user", "external-workspace", "external-token");
        let codex_home = root.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let operation = state.acquire_operation().expect("operation");
        state
            .prepare_switch_under_operation(
                &operation,
                PendingSwitch {
                    target_id: target.id.clone(),
                    target_identity: identity,
                    previous_active_id: None,
                    secret_ref: None,
                    secret_generation: 0,
                    previous_auth: Some(previous.clone()),
                    stage: PendingSwitchStage::Prepared,
                },
            )
            .expect("pending");
        codex::write_auth_document(&codex_home, &external).expect("external write");

        restore_previous_if_unchanged_with(
            &state,
            &operation,
            &codex_home,
            &written,
            Some(previous),
            || Ok(()),
        )
        .expect_err("must preserve external change");

        assert_eq!(
            codex::read_auth_document(&codex_home).expect("current"),
            external
        );
        assert!(state
            .pending_switch_under_operation(&operation)
            .expect("pending")
            .is_some());
        drop(operation);
        let _ = fs::remove_dir_all(root);
    }
}
