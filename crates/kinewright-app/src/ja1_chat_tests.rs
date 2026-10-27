#![cfg(target_os = "linux")]
use super::*;

impl HostChat {
    pub(crate) fn observe_for_test(client: HostClient, session: u64) -> Self {
        Self {
            client,
            id: Some(session),
            pending: None,
            last_poll: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            running: true,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}
