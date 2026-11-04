//! C6: Claude Code's subscription sign-in, inspected by mode fields only, and
//! the provider's private per-attempt store.
//!
//! The person's store (`~/.claude/.credentials.json`) also holds unrelated
//! credentials, such as MCP sign-ins, and a refresh token that rotates when
//! used. The provider never sees it. The host copies only the subscription
//! access token, its expiry, scopes and plan into a private store beside the
//! attempt's scratch, without the refresh token. No refresh can happen inside
//! the sandbox, so none can rotate the person's sign-in or write outside the
//! private store. A sign-in too close to expiry is refused before launch.
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Environment names that select API-key, bearer-token or third-party billing.
/// The final spawn clears every one of them; a host that has any set is
/// refused anyway, so a person who asked for API billing is never silently
/// moved onto their subscription.
pub(crate) const API_OVERRIDES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "AWS_BEARER_TOKEN_BEDROCK",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
];

/// Plans that bill a Claude subscription. Any other value, a missing plan, or
/// a missing `user:inference` scope is an unknown mode.
const SUBSCRIPTIONS: &[&str] = &["pro", "max", "team", "enterprise"];

/// The shortest remaining sign-in lifetime a launch accepts. Claude refreshes
/// a token a few minutes before it expires; this keeps a wide margin past that,
/// so a qualified run never needs the refresh token it does not have.
pub(crate) const MIN_SIGN_IN_LIFETIME_MS: u64 = 45 * 60 * 1000;

/// Fixed refusals. None carries a credential byte or a parser location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// An API key, helper, bearer token or third-party provider is configured.
    Api,
    /// No subscription sign-in, or a plan, scope or shape this release does not know.
    Unknown,
    /// The subscription sign-in expires within [`MIN_SIGN_IN_LIFETIME_MS`].
    Expiring,
    /// The store could not be read or written as qualified.
    Store,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Api => "Sign in: Claude API-key or third-party billing refused",
            Self::Unknown => "Sign in: Claude subscription sign-in required",
            Self::Expiring => "Sign in: Claude sign-in expires soon; open Claude Code to renew it",
            Self::Store => "Sign in: Claude credential store refused",
        })
    }
}
impl From<Refusal> for io::Error {
    fn from(refusal: Refusal) -> Self {
        io::Error::other(refusal.to_string())
    }
}

/// The person's Claude configuration folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn config_root() -> io::Result<PathBuf> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
        .ok_or_else(|| io::Error::other("Sign in: credential location unavailable"))?;
    fs::canonicalize(root)
}

pub(crate) fn source_path() -> io::Result<PathBuf> {
    Ok(config_root()?.join(".credentials.json"))
}

/// Claude keeps its global settings in `~/.claude.json`, or inside
/// `CLAUDE_CONFIG_DIR` when that is set.
fn global_config() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(root).join(".claude.json"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude.json"))
}

/// What the probe proved: the store's identity and the sign-in's expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignIn {
    pub identity: [u64; 2],
    pub expires_at_ms: u64,
}

/// Mode fields of one subscription sign-in. The access token is held only
/// long enough to write the private store.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    access_token: String,
    expires_at: u64,
    scopes: Vec<String>,
    #[serde(rename = "subscriptionType")]
    plan: Option<String>,
}
/// Every key other than `claudeAiOauth` is skipped unread, including MCP
/// sign-ins. The refresh token is not a field, so it is skipped too.
#[derive(serde::Deserialize)]
struct Store {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<Subscription>,
    #[serde(rename = "primaryApiKey")]
    api_key: Option<serde::de::IgnoredAny>,
}
/// The private store holds exactly one sign-in and nothing else.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateStore {
    #[serde(rename = "claudeAiOauth")]
    oauth: PrivateSubscription,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateSubscription {
    access_token: String,
    expires_at: u64,
    scopes: Vec<String>,
    subscription_type: Option<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn validate(subscription: &Subscription, now_ms: u64) -> Result<(), Refusal> {
    let token = &subscription.access_token;
    if token.is_empty()
        || token.len() > 4096
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"')
        || subscription.scopes.len() > 32
        || !subscription
            .scopes
            .iter()
            .any(|scope| scope == "user:inference")
        || !subscription
            .plan
            .as_deref()
            .is_some_and(|plan| SUBSCRIPTIONS.contains(&plan))
    {
        return Err(Refusal::Unknown);
    }
    if subscription.expires_at < now_ms.saturating_add(MIN_SIGN_IN_LIFETIME_MS) {
        return Err(Refusal::Expiring);
    }
    Ok(())
}

fn read_capped(path: &Path, cap: u64) -> Result<Option<Vec<u8>>, Refusal> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Refusal::Store),
    };
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Store)?;
    if bytes.len() as u64 > cap {
        return Err(Refusal::Store);
    }
    Ok(Some(bytes))
}

/// Settings that ask Claude for API billing: a key helper, a forced Console
/// login, or API overrides in the settings' own environment block.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::zero_sized_map_values,
    reason = "only the keys are read; every value is skipped unread"
)]
struct Settings {
    api_key_helper: Option<serde::de::IgnoredAny>,
    force_login_method: Option<String>,
    env: Option<std::collections::BTreeMap<String, serde::de::IgnoredAny>>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GlobalConfig {
    primary_api_key: Option<serde::de::IgnoredAny>,
}

/// Refuses every configured non-subscription mode before the store is read.
fn refuse_api_modes(
    config_root: &Path,
    global_config: Option<&Path>,
    managed: &Path,
    environment: &dyn Fn(&str) -> bool,
) -> Result<(), Refusal> {
    if API_OVERRIDES.iter().any(|name| environment(name)) {
        return Err(Refusal::Api);
    }
    // A managed policy applies over --settings; no managed contract is qualified.
    if fs::symlink_metadata(managed).is_ok() {
        return Err(Refusal::Unknown);
    }
    if let Some(bytes) = read_capped(&config_root.join("settings.json"), 1024 * 1024)? {
        let settings: Settings = serde_json::from_slice(&bytes).map_err(|_| Refusal::Unknown)?;
        if settings.api_key_helper.is_some()
            || settings
                .force_login_method
                .as_deref()
                .is_some_and(|method| method != "claudeai")
            || settings
                .env
                .is_some_and(|env| API_OVERRIDES.iter().any(|name| env.contains_key(*name)))
        {
            return Err(Refusal::Api);
        }
    }
    if let Some(path) = global_config
        && let Some(bytes) = read_capped(path, 16 * 1024 * 1024)?
    {
        let config: GlobalConfig = serde_json::from_slice(&bytes).map_err(|_| Refusal::Unknown)?;
        if config.primary_api_key.is_some() {
            return Err(Refusal::Api);
        }
    }
    Ok(())
}

fn read_subscription(
    path: &Path,
    identity: Option<[u64; 2]>,
) -> Result<(Subscription, [u64; 2]), Refusal> {
    let file = kinewright_job_sys::pin_credential_file(path, identity).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Refusal::Unknown
        } else {
            Refusal::Store
        }
    })?;
    let metadata = file.metadata().map_err(|_| Refusal::Store)?;
    if metadata.len() > 64 * 1024 || metadata.nlink() != 1 {
        return Err(Refusal::Store);
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Store)?;
    // Parser errors are discarded whole: they can quote input.
    let store: Store = serde_json::from_slice(&bytes).map_err(|_| Refusal::Unknown)?;
    bytes.fill(0);
    if store.api_key.is_some() {
        return Err(Refusal::Api);
    }
    let subscription = store.oauth.ok_or(Refusal::Unknown)?;
    Ok((subscription, [metadata.dev(), metadata.ino()]))
}

/// Proves a subscription sign-in in the person's store and that no
/// non-subscription mode is configured.
pub(crate) fn inspect(path: &Path, identity: Option<[u64; 2]>) -> Result<SignIn, Refusal> {
    let root = path.parent().ok_or(Refusal::Store)?;
    refuse_api_modes(
        root,
        global_config().as_deref(),
        Path::new("/etc/claude-code"),
        &|name| std::env::var_os(name).is_some(),
    )?;
    inspect_store(path, identity, now_ms())
}

fn inspect_store(path: &Path, identity: Option<[u64; 2]>, now: u64) -> Result<SignIn, Refusal> {
    let (subscription, identity) = read_subscription(path, identity)?;
    validate(&subscription, now)?;
    Ok(SignIn {
        identity,
        expires_at_ms: subscription.expires_at,
    })
}

/// Writes, or on a recheck verifies, `<scratch>/claude-config`: a private
/// folder whose only file is `.credentials.json` holding the one
/// subscription sign-in, without its refresh token.
pub(crate) fn prepare_store(
    scratch: &Path,
    source: &Path,
    identity: [u64; 2],
) -> Result<PathBuf, Refusal> {
    prepare_store_at(scratch, source, identity, now_ms())
}

fn prepare_store_at(
    scratch: &Path,
    source: &Path,
    identity: [u64; 2],
    now: u64,
) -> Result<PathBuf, Refusal> {
    let root = scratch.join("claude-config");
    match fs::create_dir(&root) {
        Ok(()) => {
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|_| Refusal::Store)?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Refusal::Store),
    }
    let meta = fs::symlink_metadata(&root).map_err(|_| Refusal::Store)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        return Err(Refusal::Store);
    }
    let path = root.join(".credentials.json");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(&path)
    {
        Ok(mut file) => {
            let (subscription, _) = read_subscription(source, Some(identity))?;
            validate(&subscription, now)?;
            let mut bytes = serde_json::to_vec(&PrivateStore {
                oauth: PrivateSubscription {
                    access_token: subscription.access_token,
                    expires_at: subscription.expires_at,
                    scopes: subscription.scopes,
                    subscription_type: subscription.plan,
                },
            })
            .map_err(|_| Refusal::Store)?;
            let written = file.write_all(&bytes).and_then(|()| file.sync_all());
            bytes.fill(0);
            written.map_err(|_| Refusal::Store)?;
            fs::File::open(&root)
                .and_then(|folder| folder.sync_all())
                .map_err(|_| Refusal::Store)?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Refusal::Store),
    }
    verify_store(&root, now)?;
    Ok(root)
}

/// The private store is exactly one pinned file of exactly the private shape.
fn verify_store(root: &Path, now: u64) -> Result<(), Refusal> {
    let path = root.join(".credentials.json");
    let file = kinewright_job_sys::pin_credential_file(&path, None).map_err(|_| Refusal::Store)?;
    let metadata = file.metadata().map_err(|_| Refusal::Store)?;
    if metadata.len() > 64 * 1024 || metadata.nlink() != 1 {
        return Err(Refusal::Store);
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Store)?;
    let store: PrivateStore = serde_json::from_slice(&bytes).map_err(|_| Refusal::Store)?;
    bytes.fill(0);
    validate(
        &Subscription {
            access_token: store.oauth.access_token,
            expires_at: store.oauth.expires_at,
            scopes: store.oauth.scopes,
            plan: store.oauth.subscription_type,
        },
        now,
    )
}

/// The private store a recipe launches with, before the provider writes to it.
pub(crate) fn verify_private_store(root: &Path) -> io::Result<()> {
    verify_store(root, now_ms()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    fn store(json: &serde_json::Value, root: &Path) -> (PathBuf, [u64; 2]) {
        let path = root.join(".credentials.json");
        let _ = fs::remove_file(&path);
        fs::write(&path, serde_json::to_vec(json).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let meta = fs::metadata(&path).unwrap();
        (path, [meta.dev(), meta.ino()])
    }
    fn subscription(plan: &str, expires: u64) -> serde_json::Value {
        // Synthetic only: never a real sign-in.
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "synthetic-access",
                "refreshToken": "synthetic-refresh-must-not-leave",
                "expiresAt": expires,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": plan,
                "rateLimitTier": "synthetic"
            },
            "mcpOAuth": {"unrelated": {"accessToken": "synthetic-unrelated-must-not-leave"}}
        })
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn s4b_c6_claude_subscription_only_and_private_store_without_refresh() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c6-auth",
        );
        let person = temp.path("person");
        fs::create_dir(&person).unwrap();
        let good = subscription("max", NOW + MIN_SIGN_IN_LIFETIME_MS + 1);
        let (path, identity) = store(&good, &person);
        let none = |_: &str| false;
        let managed = temp.path("managed-absent");
        refuse_api_modes(&person, None, &managed, &none).unwrap();
        assert_eq!(
            inspect_store(&path, None, NOW).unwrap().identity,
            identity,
            "C6_SUBSCRIPTION_ACCEPTED"
        );
        // Every non-subscription or unknown mode is refused, each by a fixed reason.
        for (json, refusal) in [
            (
                subscription("max", NOW + MIN_SIGN_IN_LIFETIME_MS - 1),
                Refusal::Expiring,
            ),
            (
                subscription("console", NOW + MIN_SIGN_IN_LIFETIME_MS + 1),
                Refusal::Unknown,
            ),
            (
                serde_json::json!({"claudeAiOauth":{"accessToken":"synthetic","expiresAt":NOW*2,"scopes":["user:inference"],"subscriptionType":null}}),
                Refusal::Unknown,
            ),
            (
                serde_json::json!({"claudeAiOauth":{"accessToken":"synthetic","expiresAt":NOW*2,"scopes":["user:profile"],"subscriptionType":"max"}}),
                Refusal::Unknown,
            ),
            (
                serde_json::json!({"primaryApiKey":"synthetic-key","claudeAiOauth":good["claudeAiOauth"]}),
                Refusal::Api,
            ),
            (serde_json::json!({"mcpOAuth":{}}), Refusal::Unknown),
        ] {
            let (path, _) = store(&json, &person);
            assert_eq!(
                inspect_store(&path, None, NOW).err(),
                Some(refusal),
                "C6_MODE_REFUSED {refusal:?}"
            );
        }
        fs::write(temp.path("torn"), b"{\"claudeAiOauth\":").unwrap();
        fs::set_permissions(temp.path("torn"), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            inspect_store(&temp.path("torn"), None, NOW).err(),
            Some(Refusal::Unknown)
        );
        // API keys and helpers in the environment or configuration refuse.
        let api = |name: &str| name == "ANTHROPIC_API_KEY";
        assert_eq!(
            refuse_api_modes(&person, None, &managed, &api),
            Err(Refusal::Api),
            "C6_API_KEY_ENV_REFUSED"
        );
        for settings in [
            serde_json::json!({"apiKeyHelper":"/usr/bin/false"}),
            serde_json::json!({"forceLoginMethod":"console"}),
            serde_json::json!({"env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}),
        ] {
            fs::write(person.join("settings.json"), settings.to_string()).unwrap();
            assert_eq!(
                refuse_api_modes(&person, None, &managed, &none),
                Err(Refusal::Api),
                "C6_HELPER_OR_THIRD_PARTY_REFUSED {settings}"
            );
        }
        fs::write(
            person.join("settings.json"),
            br#"{"theme":"dark","hooks":{}}"#,
        )
        .unwrap();
        refuse_api_modes(&person, None, &managed, &none).unwrap();
        let global = temp.path("global.json");
        fs::write(&global, br#"{"primaryApiKey":"synthetic-console-key"}"#).unwrap();
        assert_eq!(
            refuse_api_modes(&person, Some(&global), &managed, &none),
            Err(Refusal::Api),
            "C6_CONSOLE_KEY_REFUSED"
        );
        fs::create_dir(&managed).unwrap();
        assert_eq!(
            refuse_api_modes(&person, None, &managed, &none),
            Err(Refusal::Unknown),
            "C6_MANAGED_POLICY_REFUSED"
        );

        // The private store holds one sign-in, no refresh token and none of
        // the person's other credentials; the person's store is unchanged.
        let (path, identity) = store(&good, &person);
        let before = (
            fs::read(&path).unwrap(),
            fs::metadata(&path).unwrap().mtime(),
        );
        let scratch = temp.path("scratch");
        fs::create_dir(&scratch).unwrap();
        let root = prepare_store_at(&scratch, &path, identity, NOW)
            .expect("C6_PRIVATE_STORE_PREPARED (a store of any other shape is refused)");
        assert_eq!(root, scratch.join("claude-config"));
        let names: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [".credentials.json"], "C6_PRIVATE_STORE_ONE_FILE");
        assert_eq!(
            fs::metadata(&root).unwrap().mode() & 0o777,
            0o700,
            "C6_PRIVATE_STORE_MODE"
        );
        let written = fs::read(root.join(".credentials.json")).unwrap();
        let text = String::from_utf8_lossy(&written);
        assert!(
            !text.contains("refresh") && !text.contains("unrelated") && !text.contains("mcpOAuth"),
            "C6_NO_REFRESH_TOKEN_OR_UNRELATED_CREDENTIAL_COPIED"
        );
        assert_eq!(
            (
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().mtime()
            ),
            before,
            "C6_PERSON_STORE_UNCHANGED"
        );
        // A recheck verifies the same store and never rewrites it.
        assert_eq!(
            prepare_store_at(&scratch, &path, identity, NOW).unwrap(),
            root
        );
        // A changed source identity refuses a fresh store.
        let other = temp.path("other-scratch");
        fs::create_dir(&other).unwrap();
        assert_eq!(
            prepare_store_at(&other, &path, [identity[0], identity[1] + 1], NOW).err(),
            Some(Refusal::Store)
        );
        // A store the provider rewrote with a refresh token is refused.
        fs::write(
            root.join(".credentials.json"),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();
        assert_eq!(verify_store(&root, NOW).err(), Some(Refusal::Store));
    }
}
