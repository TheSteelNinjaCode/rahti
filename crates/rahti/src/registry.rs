//! The rpcs that are found at link time rather than at build time.
//!
//! A page's rpcs are known when `rahti-build` walks `src/app/`, so they are
//! dispatched by a `match` the codegen writes — the fastest possible answer,
//! checked by the compiler. A *library's* rpcs are not: the crate is compiled
//! separately, its source sits in the cargo registry, and no scan of this
//! project will ever see it. A component library could therefore ship a
//! button that renders and a `#[rpc]` that could never be called.
//!
//! So `#[rpc]` also submits an entry here. The linker gathers them from every
//! crate in the binary, this one included, and the fallback dispatcher looks
//! the name up at runtime. What is lost is the compiler's exhaustiveness
//! check over rpc names; what is gained is that a component library can ship
//! behaviour instead of only markup.
//!
//! Page rpcs are deliberately *not* reachable this way — see [`dispatch`].

use std::future::Future;
use std::pin::Pin;

use axum::extract::Request;
use axum::response::Response;

#[cfg(test)]
#[path = "tests/registry.rs"]
mod tests;

/// The shim `#[rpc]` writes, type-erased so entries of different signatures
/// sit in one collection.
pub type RpcHandler = fn(Request) -> Pin<Box<dyn Future<Output = Response> + Send>>;

/// One `#[rpc]`, submitted by the macro at the point it is written.
pub struct RpcEntry {
    /// The name the browser sends, which is the function's own.
    pub name: &'static str,
    /// Where it was written, for the message when two of them collide.
    pub source: &'static str,
    pub handler: RpcHandler,
}

inventory::collect!(RpcEntry);

/// Every registered rpc, in whatever order the linker produced.
pub fn entries() -> impl Iterator<Item = &'static RpcEntry> {
    inventory::iter::<RpcEntry>.into_iter()
}

/// Answer `name` from the registry, or say it is unknown.
///
/// `reserved` is the app's own page rpc names, which the codegen knows and
/// this does not. They are excluded because a page's rpc belongs to that
/// page's URL: without this, any page could call any other page's rpc by
/// falling through to here, which is neither what the build-time dispatcher
/// does nor what the author of a `page.rs` would expect.
pub async fn dispatch(name: &str, reserved: &[&str], req: Request) -> Response {
    if reserved.contains(&name) {
        return crate::rpc_unknown(name);
    }

    match entries().find(|entry| entry.name == name) {
        Some(entry) => (entry.handler)(req).await,
        None => crate::rpc_unknown(name),
    }
}

/// Refuse a binary in which two rpcs answer to one name.
///
/// The names are a flat namespace because the wire is: the client posts a
/// name and nothing else. Two crates can each declare `submit` without either
/// author doing anything wrong, and whichever the linker happened to place
/// first would answer for both — silently, and differently between builds.
///
/// Checked once at startup rather than on every call, and fatal rather than
/// logged: the alternative is an app that works on the machine it was built
/// on and misroutes somewhere else.
pub fn check_rpc_names() {
    let all: Vec<&RpcEntry> = entries().collect();
    if let Some((first, second)) = duplicate(&all) {
        panic!(
            "rahti: two rpcs are named `{}`.\n  {}\n  {}\n  \
             The client posts a name and nothing else, so the names have to \
             differ across everything linked into this binary — including the \
             component libraries it depends on.",
            first.name, first.source, second.source
        );
    }
}

/// The first pair sharing a name, if there is one.
///
/// Separated from the panic so the rule can be tested without a binary that
/// refuses to start.
fn duplicate<'a>(entries: &[&'a RpcEntry]) -> Option<(&'a RpcEntry, &'a RpcEntry)> {
    let mut seen: Vec<&'a RpcEntry> = Vec::new();
    for entry in entries {
        if let Some(other) = seen.iter().find(|s| s.name == entry.name) {
            return Some((other, entry));
        }
        seen.push(entry);
    }
    None
}
