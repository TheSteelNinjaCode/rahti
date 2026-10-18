//! Rahti's runtime.
//!
//! `rahti-build` runs at build time and produces the app's `src/routes.rs`;
//! this crate is what that generated code, and every `page.rs`, actually
//! calls.
//!
//! The `html!` macro expands to `::rahti::` paths — absolute, naming this
//! crate itself rather than a path inside whoever is compiling — so the
//! expansion compiles the same wherever it is written.

// Framework surface: exported for apps to use, so "nothing in this crate
// calls it yet" is not a defect.
#![allow(dead_code, unused_imports)]

mod attrs;
pub mod auth;
pub mod authorization;
#[cfg(feature = "browser-test")]
pub mod browser;
pub mod cache;
mod dev;
mod env;
mod error;
mod html;
pub mod i18n;
pub mod jobs;
mod layout;
mod listen;
mod loading;
mod metadata;
pub mod observability;
pub mod rate_limit;
mod registry;
pub mod request_pipeline;
mod rpc;
pub mod schedule;
pub mod security;
mod shutdown;
mod static_export;
mod statics;
pub mod time;
mod upload;
pub mod validation;
#[cfg(feature = "ws")]
pub mod ws;
#[cfg(feature = "mcp")]
pub use rahti_mcp as mcp;

/// The auth layer, under the name generated code calls it by — beside `csrf`
/// and `revalidate`, which is where it sits in the router.
/// Attributes decided at runtime: whether one is written, and what a
/// `@{..spread}` brings. `AttrList` and `attribute` are what `html!` expands
/// into; `Attrs` and `Attributes` are what an application writes.
pub use attrs::{AttrList, Attributes, Attrs, Bindings, Presence, attribute};
pub use auth::guard as auth_guard;
pub use dev::{DEV_ENV, revalidate, routes as dev_routes};
/// `.env` into the process environment, once. Called by `AuthSettings::from_env`
/// and by the application's `db::connect`, so a project reaches its variables
/// whether or not it has a database.
pub use env::load_env;
pub use error::{Error, ErrorInfo, ErrorSource, Result, apply_error, panic_response};
pub use html::{Html, Json, PageBody, Render, RenderJs, render_deferred_children};
pub use layout::{
    ROOT_LAYOUT_HEADER, apply_layout, apply_layout_metadata, apply_root_layout,
    apply_root_layout_metadata, root_document, root_document_metadata,
};
/// The listener `main` serves on. Binds the configured address; in dev mode a
/// busy port falls forward to the next free one, in release it is an error.
/// `local_url` is where to open a browser for it — a wildcard bind address is
/// not somewhere a browser can go, so it is named as `localhost`.
pub use listen::{listen, local_url};
/// The app's `loading.rs` files, installed once by the generated router.
pub use loading::{
    REGION_ID as LOADING_REGION_ID, URL_ATTR as LOADING_URL_ATTR, install as install_loading,
};
pub use metadata::{Metadata, Page, Robots, apply_metadata, merge_metadata};
/// The `#[socket]` attribute, behind the same feature as the module it
/// expands against: without `ws` the attribute would write code that names
/// `rahti::ws`, and the error should say the feature is off rather than
/// point into generated tokens.
#[cfg(feature = "ws")]
pub use rahti_macros::socket;
pub use rahti_macros::{component, html, rpc};
pub use rate_limit::{middleware as rate_limit, state as rate_limit_state};
pub use registry::{RpcEntry, RpcHandler, check_rpc_names, dispatch as dispatch_registered};
pub use rpc::{
    RpcBody, RpcCall, RpcResponse, RpcSender, RpcStream, csrf, rpc_failed, rpc_name, rpc_ok,
    rpc_panic_response, rpc_stream, rpc_unknown,
};
pub use security::middleware as security_headers;
pub use shutdown::{
    Shutdown, begin as begin_shutdown, signal as shutdown_signal, watch as watch_shutdown,
};
pub use static_export::{
    StaticExportError, StaticExportReport, StaticExportSkip, StaticRoute, export_static,
    static_output,
};
pub use statics::{PUBLIC_DIR_ENV, public_dir};
pub use upload::{
    DEFAULT_MAX_FILE, DEFAULT_MAX_UPLOAD, DEFAULT_SPILL_THRESHOLD, MAX_FILE_ENV, MAX_UPLOAD_ENV,
    RpcFile, RpcFileReader, RpcUpload, SPILL_DIR_ENV, SPILL_THRESHOLD_ENV, max_file, max_upload,
    spill_dir, spill_threshold,
};
pub use validation::{VALIDATION_STATUS, Validate, ValidationErrors, ValidationResult, Validator};

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
/// whose parameter is an axum request, and an application that only depends
/// on `rahti` must not have to add axum to its own manifest to use an
/// attribute this crate provides.
pub use axum;
