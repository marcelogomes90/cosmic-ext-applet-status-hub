#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use cosmic_status_hub::core::model::TraySnapshot;
use tokio::sync::watch;

pub const SETTLE: Duration = Duration::from_secs(15);

pub type Snapshots = watch::Receiver<Arc<TraySnapshot>>;

pub fn init() {
    cosmic_status_hub::init_tracing();
}

pub async fn wait_for(
    snapshots: &mut Snapshots,
    what: &str,
    mut predicate: impl FnMut(&TraySnapshot) -> bool,
) -> Arc<TraySnapshot> {
    let deadline = tokio::time::Instant::now() + SETTLE;

    loop {
        {
            let current = snapshots.borrow_and_update().clone();
            if predicate(&current) {
                return current;
            }
        }

        if tokio::time::timeout_at(deadline, snapshots.changed())
            .await
            .is_err()
        {
            let last = snapshots.borrow().clone();
            panic!(
                "timed out waiting for {what}; last snapshot was {:?}",
                describe(&last)
            );
        }
    }
}

pub async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + SETTLE;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn keys(snapshot: &TraySnapshot) -> Vec<String> {
    snapshot
        .items
        .iter()
        .map(|item| item.key.to_string())
        .collect()
}

fn describe(snapshot: &TraySnapshot) -> Vec<String> {
    snapshot
        .items
        .iter()
        .map(|item| format!("{} ({}) [{}]", item.key, item.state, item.address))
        .collect()
}
