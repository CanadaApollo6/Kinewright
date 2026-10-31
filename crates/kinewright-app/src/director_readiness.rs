//! Local installation and sign-in readiness never establish release certification.
use kinewright_core::{AuthenticationStatus, HarnessInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    NotAvailable,
    CheckAgain,
    Install,
    SignIn,
    LocalSignIn,
}
pub(crate) fn readiness(qualified: bool, detected: bool, info: Option<&HarnessInfo>) -> Readiness {
    if !qualified {
        return Readiness::NotAvailable;
    }
    if !detected {
        return Readiness::CheckAgain;
    }
    match info {
        None => Readiness::Install,
        Some(info) => match info.authentication {
            AuthenticationStatus::Unauthenticated => Readiness::SignIn,
            AuthenticationStatus::Unknown => Readiness::CheckAgain,
            AuthenticationStatus::Authenticated => Readiness::LocalSignIn,
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s4b_c4_local_readiness_never_substitutes_for_qualification() {
        let mut info = HarnessInfo {
            id: kinewright_core::HarnessId::new("codex"),
            executable: "fixture-cli".into(),
            version: Some("fixture".into()),
            authentication: AuthenticationStatus::Authenticated,
            subscription_tier: None,
        };
        assert_eq!(readiness(false, true, Some(&info)), Readiness::NotAvailable);
        assert_eq!(readiness(false, true, None), Readiness::NotAvailable);
        assert_eq!(readiness(true, false, None), Readiness::CheckAgain);
        assert_eq!(readiness(true, true, None), Readiness::Install);
        info.authentication = AuthenticationStatus::Unauthenticated;
        assert_eq!(readiness(true, true, Some(&info)), Readiness::SignIn);
        info.authentication = AuthenticationStatus::Unknown;
        assert_eq!(readiness(true, true, Some(&info)), Readiness::CheckAgain);
        info.authentication = AuthenticationStatus::Authenticated;
        assert_eq!(readiness(true, true, Some(&info)), Readiness::LocalSignIn);
    }
}
