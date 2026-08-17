//! Rahti's runtime.
//!
//! `rahti-build` runs at build time and produces the app's `src/routes.rs`;
//! this crate is what that generated code, and every `page.rs`, actually
//! calls.
//!
//! The `html!` macro expands to `::rahti::` paths — absolute, naming this
//! crate itself rather than a path inside whoever is compiling. Any crate
//! that depends on `rahti` can therefore write components, which is what
//! makes a component library on crates.io possible: a dependency has no
//! `crate::rahti` to offer.

// Framework surface: exported for apps to use, so "nothing in this crate
// calls it yet" is not a defect.
#![allow(dead_code, unused_imports)]

mod attrs;
pub mod auth;
mod dev;
mod env;
mod error;
mod html;
mod layout;
mod listen;
mod loading;
mod registry;
mod rpc;
mod shutdown;
mod statics;
mod upload;
#[cfg(feature = "ws")]
pub mod ws;

/// The auth layer, under the name generated code calls it by — beside `csrf`
/// and `revalidate`, which is where it sits in the router.
/// Attributes decided at runtime: whether one is written, and what a
/// `@{..spread}` brings. `AttrList` and `attribute` are what `html!` expands
/// into; `Attrs` and `Attributes` are what an application writes.
pub use attrs::{AttrList, Attributes, Attrs, Presence, attribute};
pub use auth::guard as auth_guard;
pub use dev::{DEV_ENV, revalidate, routes as dev_routes};
/// `.env` into the process environment, once. Called by `AuthSettings::from_env`
/// and by the application's `db::connect`, so a project reaches its variables
/// whether or not it has a database.
pub use env::load_env;
pub use error::{Error, ErrorInfo, ErrorSource, Result, apply_error, panic_response};
pub use html::{Html, Json, PageBody, Render, RenderJs};
pub use layout::{ROOT_LAYOUT_HEADER, apply_layout, apply_root_layout, root_document};
/// The listener `main` serves on. Binds the configured address; in dev mode a
/// busy port falls forward to the next free one, in release it is an error.
/// `local_url` is where to open a browser for it — a wildcard bind address is
/// not somewhere a browser can go, so it is named as `localhost`.
pub use listen::{listen, local_url};
/// The app's `loading.rs` files, installed once by the generated router.
pub use loading::{
    REGION_ID as LOADING_REGION_ID, URL_ATTR as LOADING_URL_ATTR, install as install_loading,
};
/// The `#[socket]` attribute, behind the same feature as the module it
/// expands against: without `ws` the attribute would write code that names
/// `rahti::ws`, and the error should say the feature is off rather than
/// point into generated tokens.
#[cfg(feature = "ws")]
pub use rahti_macros::socket;
pub use rahti_macros::{component, html, rpc};
pub use registry::{RpcEntry, RpcHandler, check_rpc_names, dispatch as dispatch_registered};
pub use rpc::{
    RpcBody, RpcCall, RpcResponse, RpcSender, RpcStream, csrf, rpc_failed, rpc_name, rpc_ok,
    rpc_panic_response, rpc_stream, rpc_unknown,
};
pub use shutdown::{
    Shutdown, begin as begin_shutdown, signal as shutdown_signal, watch as watch_shutdown,
};
pub use statics::{PUBLIC_DIR_ENV, public_dir};
pub use upload::{
    DEFAULT_MAX_FILE, DEFAULT_MAX_UPLOAD, DEFAULT_SPILL_THRESHOLD, MAX_FILE_ENV, MAX_UPLOAD_ENV,
    RpcFile, RpcFileReader, RpcUpload, SPILL_DIR_ENV, SPILL_THRESHOLD_ENV, max_file, max_upload,
    spill_dir, spill_threshold,
};

/// Serialization, for the values an `#[rpc]` sends back.
///
/// Re-exported so a page can name the JSON types it answers with without the
/// app having to depend on serde directly — and so generated code has one
/// path to reach it by.
pub use serde_json;

/// Re-exported so `#[rpc]` can submit a registry entry from any crate that
/// depends on `rahti`, without that crate having to depend on `inventory`
/// itself — the same reason the macros are re-exported here.
pub use inventory;

/// Re-exported for the same reason as the macros: `#[rpc]` writes a shim
/// whose parameter is an axum request, and a component library that only
/// depends on `rahti` must not have to add axum to its own manifest to use
/// an attribute this crate provides.
pub use axum;
