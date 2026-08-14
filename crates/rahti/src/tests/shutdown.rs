//! Stopping, and why a subscription is taken before it is awaited.

use super::*;
use std::time::Duration;

/// Anything holding a response open ends it when its `Shutdown` fires. The
/// dev event stream is the one that matters: `axum::serve` waits for every
/// open connection, and that stream never closes on its own.
#[tokio::test]
async fn a_watcher_is_released_when_shutdown_starts() {
    let shutdown = watch();
    let _ = channel().send(());

    tokio::time::timeout(Duration::from_secs(5), shutdown.fired())
        .await
        .expect("shutdown did not reach the watcher");
}

/// Every watcher hears it, however many are open.
#[tokio::test]
async fn every_watcher_hears_it() {
    let watchers: Vec<Shutdown> = (0..4).map(|_| watch()).collect();
    let _ = channel().send(());

    for shutdown in watchers {
        tokio::time::timeout(Duration::from_secs(5), shutdown.fired())
            .await
            .expect("a watcher was missed");
    }
}

/// A watcher taken before the signal hears it however long it waits, which
/// is the reason `watch` runs while a request is being set up rather than at
/// the moment of the wait. The opposite case — a subscription taken too late
/// missing the signal — is not asserted here: the channel is process-wide, so
/// a test asserting that nothing arrives would be answering other tests' sends.
#[tokio::test]
async fn a_watcher_that_waits_a_while_still_hears_it() {
    let shutdown = watch();
    let _ = channel().send(());
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::timeout(Duration::from_secs(5), shutdown.fired())
        .await
        .expect("a signal sent before the wait was lost");
}

/// Sending with nothing subscribed is the common case, not an error.
#[test]
fn stopping_with_no_watchers_is_fine() {
    let _ = channel().send(());
}
