//! Stopping.
//!
//! `axum::serve` shuts down gracefully by waiting for every open connection
//! to finish, which is right for a request that is still being answered and
//! wrong for one that is never going to finish on its own. The dev SSE stream
//! is the second kind: it stays open for as long as a browser sits on the
//! page, so Ctrl+C would print its message and then hang until the terminal
//! killed the process.
//!
//! So stopping is broadcast, not just awaited. [`signal`] resolves on Ctrl+C
//! and tells the rest of the process; anything holding a response open ends
//! it when its [`Shutdown`] fires.

use std::sync::OnceLock;

use tokio::sync::broadcast;

#[cfg(test)]
#[path = "tests/shutdown.rs"]
mod tests;

fn channel() -> &'static broadcast::Sender<()> {
    static CHANNEL: OnceLock<broadcast::Sender<()>> = OnceLock::new();
    // Capacity 1: the message carries no information beyond having been
    // sent, so a receiver that lags has still learned the only thing there
    // was to learn.
    CHANNEL.get_or_init(|| broadcast::channel(1).0)
}

/// Resolves once the process should stop, having told every subscriber.
///
/// Hand this to `axum::serve(..).with_graceful_shutdown(..)`.
pub async fn signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");

    begin();
}

/// Start shutting down, without waiting for Ctrl+C.
///
/// [`signal`] is the terminal's answer to "when do we stop", and it is the
/// only one a web deployment has. A packaged native application has a
/// different one: the user closed the window, or Android is destroying the
/// activity. Neither delivers `SIGINT`, and on Android nothing ever will — so
/// the host says when instead of waiting to be told.
///
/// What it does is the second half of [`signal`]: every [`Shutdown`] fires, so
/// the dev event stream and every open WebSocket end rather than holding a
/// graceful shutdown open until something kills the process.
///
/// Idempotent. A second call sends a second message to receivers that have
/// already stopped listening, which is nothing.
pub fn begin() {
    // Fails only when nothing is subscribed, which is the common case and
    // not an error.
    let _ = channel().send(());
}

/// A promise that shutdown has begun.
///
/// Take one with [`watch`] *before* awaiting it. A broadcast receiver only
/// sees what is sent after it subscribes, so subscribing at the moment of the
/// wait would race [`signal`] and could miss it outright.
pub struct Shutdown(broadcast::Receiver<()>);

impl Shutdown {
    /// Resolves when shutdown starts — or immediately if it already has.
    pub async fn fired(mut self) {
        // `Err` is a closed or lagged channel. Neither can mean "keep
        // waiting": the sender is static and never dropped, so the only way
        // here is a signal that was sent.
        let _ = self.0.recv().await;
    }
}

/// Subscribe to shutdown. Cheap, and safe to call per request.
pub fn watch() -> Shutdown {
    Shutdown(channel().subscribe())
}
