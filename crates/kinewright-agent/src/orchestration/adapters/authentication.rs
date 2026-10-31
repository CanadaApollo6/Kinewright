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
    InPlaceWrite,
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
    refresh: CredentialRefreshContract {
        operations: &[
            CredentialStoreOperation::ReadCredentials,
            CredentialStoreOperation::InPlaceWrite,
        ],
    },
};
pub const CLAUDE_AUTH_CONTRACT: ProviderAuthContract = ProviderAuthContract {
    provider: "claude",
    proof: SubscriptionProof::ClaudeSubscriptionBillingAndPrecedence,
    refresh: REFRESH,
};

/// Exact sandbox allowlist required by one qualified provider/version/OS.
/// Codex `single_file` grants one mounted inode beneath private root, never its source directory.
/// All entries are relative to root; temp prefixes authorize only siblings of credentials.
/// Keychain/managed-policy stores need a separately qualified contract before registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialFile {
    pub source: PathBuf,
    pub identity: [u64; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStoreScope {
    pub root: PathBuf,
    pub single_file: Option<CredentialFile>,
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

/// Both observations use the same effective context. Codex rechecks in-place saves;
/// Claude still requires its atomic-refresh qualification. No refresh is inferred from two reads.
/// A loggedIn flag or ambient login-status text cannot populate subscription proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveAuthEvidence {
    pub contract: ProviderAuthContract,
    pub context: EffectiveLaunchContext,
    pub at_launch: AuthenticationMode,
    /// None at Codex certification: final spawn and exit perform real rechecks.
    /// It never asserts that an actual refresh happened.
    pub at_recheck: Option<AuthenticationMode>,
    pub observed_store: CredentialStoreScope,
}
impl EffectiveAuthEvidence {
    pub(super) fn validate(
        &self,
        contract: ProviderAuthContract,
        launch: &EffectiveLaunchContext,
        executable: &ExecutableIdentity,
    ) -> Result<(), CertificationRefusal> {
        for mode in std::iter::once(self.at_launch).chain(self.at_recheck) {
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
        if (self.at_recheck.is_none()
            && contract.proof != SubscriptionProof::CodexSubscriptionLoginMethod)
            || self.contract != contract
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
            || (scope.temporary_file_prefixes.is_empty() && scope.single_file.is_none())
            || scope.temporary_file_prefixes.len() > 32
            || !scope
                .credential_files
                .iter()
                .chain(&scope.lock_files)
                .chain(&scope.temporary_file_prefixes)
                .all(relative)
            || (contract.proof == SubscriptionProof::CodexSubscriptionLoginMethod
                && (scope.credential_files != [PathBuf::from("auth.json")]
                    || !scope.lock_files.is_empty()
                    || !scope.temporary_file_prefixes.is_empty()
                    || !scope
                        .single_file
                        .as_ref()
                        .is_some_and(|file| file.source.is_absolute() && file.identity != [0, 0])))
            || scope.refresh != contract.refresh
            || self.observed_store != *scope
        {
            return Err(CertificationRefusal::EffectiveAuthentication);
        }
        Ok(())
    }
}

#[cfg(any(test, all(target_os = "linux", feature = "test-util")))]
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
            single_file: None,
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
        at_launch: AuthenticationMode::Subscription,
        at_recheck: Some(AuthenticationMode::Subscription),
        observed_store: context.credential_store.clone(),
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
            let mut context = context;
            context.credential_store.refresh = contract.refresh;
            if contract.proof == SubscriptionProof::CodexSubscriptionLoginMethod {
                context.credential_store.credential_files = vec!["auth.json".into()];
                context.credential_store.lock_files.clear();
                context.credential_store.temporary_file_prefixes.clear();
                context.credential_store.single_file = Some(CredentialFile {
                    source: std::env::current_dir().unwrap().join("synthetic-auth.json"),
                    identity: [1, 2],
                });
            }
            evidence.context = context.clone();
            evidence.observed_store = context.credential_store.clone();
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
                .observed_store
                .temporary_file_prefixes
                .push("other-store".into());
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::EffectiveAuthentication)
            );
            changed = evidence.clone();
            changed.at_recheck = Some(AuthenticationMode::ApiKey);
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::ApiAuthentication)
            );
            changed.at_recheck = Some(AuthenticationMode::Unknown);
            assert_eq!(
                changed.validate(contract, &context, &executable),
                Err(CertificationRefusal::UnknownAuthentication)
            );
        }
    }
}
