//! Proof requirements, not CLI detectors. Qualification must witness each provider's refresh.
use super::{AuthenticationMode, CertificationRefusal, ExecutableIdentity};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionProof {
    CodexSubscriptionLoginMethod,
    ClaudeSubscriptionBillingAndPrecedence,
    #[cfg(any(test, feature = "test-util"))]
    Fixture,
}

/// Every allowed write is scoped to the resolved credential store, not HOME.
/// Qualification supplies exact credential/lock names and a bounded temp-file rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreOperation {
    ReadCredentials,
    ExclusiveStoreLocks,
    CreateSiblingTemporary,
    AtomicReplace,
    SyncStoreDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialRefreshContract {
    pub operations: &'static [CredentialStoreOperation],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderAuthContract {
    pub provider: &'static str,
    pub proof: SubscriptionProof,
    pub refresh: CredentialRefreshContract,
}

const REFRESH: CredentialRefreshContract = CredentialRefreshContract {
    operations: &[
        CredentialStoreOperation::ReadCredentials,
        CredentialStoreOperation::ExclusiveStoreLocks,
        CredentialStoreOperation::CreateSiblingTemporary,
        CredentialStoreOperation::AtomicReplace,
        CredentialStoreOperation::SyncStoreDirectory,
    ],
};

/// Neither contract is release evidence or registration. C5/C6 must resolve and witness it.
pub const CODEX_AUTH_CONTRACT: ProviderAuthContract = ProviderAuthContract {
    provider: "codex",
    proof: SubscriptionProof::CodexSubscriptionLoginMethod,
    refresh: REFRESH,
};
pub const CLAUDE_AUTH_CONTRACT: ProviderAuthContract = ProviderAuthContract {
    provider: "claude",
    proof: SubscriptionProof::ClaudeSubscriptionBillingAndPrecedence,
    refresh: REFRESH,
};

/// Exact sandbox allowlist required by one qualified provider/version/OS.
/// All entries are relative to root; temp prefixes authorize only siblings of credentials.
/// Keychain/managed-policy stores need a separately qualified contract before registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStoreScope {
    pub root: PathBuf,
    pub credential_files: Vec<PathBuf>,
    pub lock_files: Vec<PathBuf>,
    pub temporary_file_prefixes: Vec<PathBuf>,
    pub refresh: CredentialRefreshContract,
}

/// Fingerprints cover the exact child-only environment and isolated settings, never their secrets.
/// Cleared means API overrides/helpers and inherited manager secrets were removed at final spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveLaunchContext {
    pub environment_digest: [u8; 32],
    pub settings_digest: [u8; 32],
    pub environment_cleared: bool,
    pub settings_isolated: bool,
    pub credential_store: CredentialStoreScope,
    pub executable: ExecutableIdentity,
}

/// Both observations must use this same effective context, including after atomic refresh.
/// A loggedIn flag or ambient login-status text cannot populate subscription proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveAuthEvidence {
    pub contract: ProviderAuthContract,
    pub context: EffectiveLaunchContext,
    pub before_refresh: AuthenticationMode,
    pub after_refresh: AuthenticationMode,
    pub refreshed_store: CredentialStoreScope,
}
impl EffectiveAuthEvidence {
    pub(super) fn validate(
        &self,
        contract: ProviderAuthContract,
        launch: &EffectiveLaunchContext,
        executable: &ExecutableIdentity,
    ) -> Result<(), CertificationRefusal> {
        for mode in [self.before_refresh, self.after_refresh] {
            match mode {
                AuthenticationMode::ApiKey => return Err(CertificationRefusal::ApiAuthentication),
                AuthenticationMode::Unknown => {
                    return Err(CertificationRefusal::UnknownAuthentication);
                }
                AuthenticationMode::Subscription => {}
            }
        }
        let scope = &launch.credential_store;
        let relative = |path: &PathBuf| {
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
        };
        if self.contract != contract
            || &self.context != launch
            || &launch.executable != executable
            || !launch.environment_cleared
            || !launch.settings_isolated
            || launch.environment_digest == [0; 32]
            || launch.settings_digest == [0; 32]
            || !scope.root.is_absolute()
            || scope.credential_files.is_empty()
            || scope.credential_files.len() > 32
            || scope.lock_files.len() > 32
            || scope.temporary_file_prefixes.is_empty()
            || scope.temporary_file_prefixes.len() > 32
            || !scope
                .credential_files
                .iter()
                .chain(&scope.lock_files)
                .chain(&scope.temporary_file_prefixes)
                .all(relative)
            || scope.refresh != contract.refresh
            || self.refreshed_store != *scope
        {
            return Err(CertificationRefusal::EffectiveAuthentication);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn fixture(
    executable: &ExecutableIdentity,
) -> (EffectiveLaunchContext, EffectiveAuthEvidence) {
    let context = EffectiveLaunchContext {
        environment_digest: [1; 32],
        settings_digest: [2; 32],
        environment_cleared: true,
        settings_isolated: true,
        executable: executable.clone(),
        credential_store: CredentialStoreScope {
            root: std::env::current_dir().unwrap().join("fixture-credentials"),
            credential_files: vec!["credentials.json".into()],
            lock_files: vec!["credentials.lock".into()],
            temporary_file_prefixes: vec!["credentials.tmp-".into()],
            refresh: REFRESH,
        },
    };
    let evidence = EffectiveAuthEvidence {
        contract: ProviderAuthContract {
            provider: "scripted",
            proof: SubscriptionProof::Fixture,
            refresh: REFRESH,
        },
        context: context.clone(),
        before_refresh: AuthenticationMode::Subscription,
        after_refresh: AuthenticationMode::Subscription,
        refreshed_store: context.credential_store.clone(),
    };
    (context, evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4b_c1_both_provider_auth_contracts_bind_effective_context_and_refresh() {
        let executable = ExecutableIdentity {
            path: std::env::current_exe().unwrap(),
            file_identity: [1, 2],
        };
        for contract in [CODEX_AUTH_CONTRACT, CLAUDE_AUTH_CONTRACT] {
            let (context, mut evidence) = fixture(&executable);
            evidence.contract = contract;
            assert_eq!(evidence.validate(contract, &context, &executable), Ok(()));
            for field in 0..7 {
                let mut changed = context.clone();
                match field {
                    0 => changed.environment_digest = [3; 32],
                    1 => changed.settings_digest = [3; 32],
                    2 => {
                        changed.credential_store.root = changed.credential_store.root.join("other");
                    }
                    3 => changed.executable.file_identity[1] += 1,
                    4 => changed.environment_cleared = false,
                    5 => changed.settings_isolated = false,
                    _ => changed
                        .credential_store
                        .credential_files
                        .push("../escape".into()),
                }
                assert_eq!(
                    evidence.validate(contract, &changed, &executable),
                    Err(CertificationRefusal::EffectiveAuthentication)
                );
            }
            let mut changed = evidence.clone();
            changed
                .refreshed_store
                .temporary_file_prefixes
                .push("other-store".into());
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::EffectiveAuthentication)
            );
            changed = evidence.clone();
            changed.after_refresh = AuthenticationMode::ApiKey;
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::ApiAuthentication)
            );
            changed.after_refresh = AuthenticationMode::Unknown;
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::UnknownAuthentication)
            );
        }
    }
}
