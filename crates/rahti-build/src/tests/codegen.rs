//! The router and the manifest the scan turns into.
//!
//! The generated file is ordinary Rust the compiler reads next, so these
//! tests assert the lines it has to contain and the order the layers go on —
//! everything a reader of `src/routes.rs` would check by eye.

use super::*;
use crate::Engine;
use crate::route::Kind;
use crate::scanner::Entry;
use crate::{Css, DEFAULT_TAILWIND};

fn config() -> Config {
    Config {
        app_dir: "src/app".to_string(),
        public_dir: "public".to_string(),
        host: "0.0.0.0".to_string(),
        port: 3000,
        // No component libraries unless a test says otherwise, so the
        // registry fallback is generated only where it is the subject.
        has_libraries: false,
        // The router is the same with a database and without one — the
        // connection is reached from a page, not routed to.
        db: None,
        // Codegen never reads this: the stylesheet is compiled beside the
        // router, not written into it.
        css: Css {
            entry: "src/app/globals.css".to_string(),
            output: "public/css/styles.css".to_string(),
            version: DEFAULT_TAILWIND.to_string(),
            download: false,
            minify: false,
            sha256: None,
            engine: Engine::Tailwind,
        },
    }
}

/// Build the entries a scan of these files would produce.
fn entries(files: &[(Kind, &str, &[&str])]) -> Vec<Entry> {
    let mut used = Vec::new();
    files
        .iter()
        .map(|(kind, file, methods)| {
            let dirs: Vec<String> = file
                .trim_start_matches("src/app/")
                .split('/')
                .map(str::to_string)
                .filter(|s| !s.ends_with(".rs"))
                .collect();
            let urls = if kind.is_routable() {
                route::to_urls(&dirs)
            } else {
                Vec::new()
            };
            let relative = file.trim_start_matches("src/app/").to_string();
            Entry {
                kind: *kind,
                file: (*file).to_string(),
                module: route::module_name(*kind, &dirs, &mut used),
                component: (*kind != Kind::Route).then(|| route::component_id(*kind, &relative)),
                methods: methods.iter().map(|m| (*m).to_string()).collect(),
                rpcs: Vec::new(),
                sockets: Vec::new(),
                // What the scan derives: `app/layout.rs` is a document root
                // because nothing sits above it. A layout that declares
                // itself one is built with `as_root`.
                root: *kind == Kind::Layout && dirs.is_empty(),
                dirs,
                urls,
            }
        })
        .collect()
}

/// The same entries, with one layout declaring itself a document root.
fn as_root(mut entries: Vec<Entry>, file: &str) -> Vec<Entry> {
    let entry = entries
        .iter_mut()
        .find(|e| e.file == file)
        .unwrap_or_else(|| panic!("no entry for {file}"));
    entry.root = true;
    entries
}

/// The same entries, with `#[rpc]` functions hung on one page.
fn with_rpcs(mut entries: Vec<Entry>, file: &str, rpcs: &[&str]) -> Vec<Entry> {
    let entry = entries
        .iter_mut()
        .find(|e| e.file == file)
        .unwrap_or_else(|| panic!("no entry for {file}"));
    entry.rpcs = rpcs.iter().map(|name| (*name).to_string()).collect();
    entries
}

/// Most of these tests have no sockets — the ordinary app — so the socket
/// list is threaded in only where it is the subject.
fn gen_routes(entries: &[Entry], component_rpcs: &[Rpc], config: &Config) -> String {
    routes_rs(entries, component_rpcs, &[], config)
}

fn gen_manifest(
    entries: &[Entry],
    components: &[String],
    component_rpcs: &[Rpc],
    database: Option<&crate::db::Scan>,
    config: &Config,
) -> String {
    manifest_json(entries, components, component_rpcs, &[], database, config)
}

/// One `#[socket]` list, the shape `socket_list` produces.
fn sockets(names: &[(&str, &str)]) -> Vec<Rpc> {
    names
        .iter()
        .map(|(name, file)| Rpc {
            name: (*name).to_string(),
            module: "socket_module".to_string(),
            file: (*file).to_string(),
        })
        .collect()
}

const PAGE: Kind = Kind::Page;
const LAYOUT: Kind = Kind::Layout;
const ERROR: Kind = Kind::Error;
const ROUTE: Kind = Kind::Route;
const NOT_FOUND: Kind = Kind::NotFound;
const LOADING: Kind = Kind::Loading;

/// The shape of the app in this repository: a root layout, a nested group, a
/// nested error boundary, an API route and a 404.
fn app() -> Vec<Entry> {
    entries(&[
        (LAYOUT, "src/app/layout.rs", &[]),
        (PAGE, "src/app/page.rs", &[]),
        (PAGE, "src/app/users/page.rs", &[]),
        (LAYOUT, "src/app/(marketing)/layout.rs", &[]),
        (PAGE, "src/app/(marketing)/adds/page.rs", &[]),
        (ERROR, "src/app/error.rs", &[]),
        (ERROR, "src/app/customers/error.rs", &[]),
        (PAGE, "src/app/customers/page.rs", &[]),
        (PAGE, "src/app/customers/[id]/page.rs", &[]),
        (ROUTE, "src/app/api/health/route.rs", &["get", "post"]),
        (NOT_FOUND, "src/app/not-found.rs", &[]),
    ])
}

/// The position of `needle`, or a failure naming what was generated.
fn at(out: &str, needle: &str) -> usize {
    out.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is missing from:\n{out}"))
}

/// The `.route(..)` line for one URL.
///
/// Matched on the call rather than on the URL alone: the doc comment above
/// `router` lists every URL too.
fn route_line_for<'a>(out: &'a str, url: &str) -> &'a str {
    let call = format!(".route(\"{url}\",");
    out.lines()
        .find(|l| l.contains(&call))
        .unwrap_or_else(|| panic!("nothing mounts `{url}`:\n{out}"))
}

/// The sub-router that mounts `url`, layers included.
fn group_for<'a>(out: &'a str, url: &str) -> &'a str {
    let call = format!(".route(\"{url}\",");
    out.split("let group_")
        .skip(1)
        .find(|g| g.contains(&call))
        .unwrap_or_else(|| panic!("no group mounts `{url}`:\n{out}"))
}

// ------------------------------------------------------------------ modules

/// `#[path]` resolves relative to `src/`, so the crate-root-relative path
/// loses its prefix.
#[test]
fn every_wired_file_gets_a_module() {
    let out = gen_routes(&app(), &[], &config());
    assert!(out.contains("#[path = \"app/page.rs\"]"), "{out}");
    assert!(
        out.contains("#[path = \"app/api/health/route.rs\"]"),
        "{out}"
    );
    assert!(out.contains("#[path = \"app/not-found.rs\"]"), "{out}");
}

/// A layout nothing inherits, and a boundary nothing routes through, would
/// only be dead code in the generated file.
#[test]
fn an_unused_layout_is_left_out() {
    let out = gen_routes(
        &entries(&[
            (PAGE, "src/app/page.rs", &[]),
            (LAYOUT, "src/app/orphan/layout.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert!(!out.contains("orphan"), "{out}");
}

#[test]
#[should_panic(expected = "lives outside `src/`")]
fn a_file_outside_src_cannot_be_reached() {
    gen_routes(&entries(&[(PAGE, "pages/page.rs", &[])]), &[], &config());
}

// ----------------------------------------------------------------- routing

#[test]
fn configuration_reaches_the_generated_file() {
    let out = gen_routes(&app(), &[], &config());
    assert!(out.contains("pub const HOST: &str = \"0.0.0.0\";"), "{out}");
    assert!(out.contains("pub const PORT: u16 = 3000;"), "{out}");
    assert!(
        out.contains("pub const PUBLIC_DIR: &str = \"public\";"),
        "{out}"
    );
}

#[test]
fn a_page_is_mounted_as_a_get() {
    let out = gen_routes(&app(), &[], &config());
    assert!(
        out.contains(".route(\"/users\", axum::routing::get("),
        "{out}"
    );
    assert!(
        out.contains(".route(\"/customers/{id}\", axum::routing::get("),
        "{out}"
    );
}

/// Every method a `route.rs` exports is chained onto one entry.
#[test]
fn a_route_mounts_each_method_it_exports() {
    let out = gen_routes(&app(), &[], &config());
    let line = route_line_for(&out, "/api/health");
    assert!(line.contains("axum::routing::get("), "{line}");
    assert!(line.contains(".post("), "{line}");
}

/// Cosmetic — axum ranks matches itself — but the file is read by people.
#[test]
fn static_routes_are_listed_before_dynamic_ones() {
    let out = gen_routes(&app(), &[], &config());
    assert!(
        at(&out, ".route(\"/users\"") < at(&out, ".route(\"/customers/{id}\""),
        "{out}"
    );
}

#[test]
fn an_optional_catch_all_is_mounted_twice() {
    let out = gen_routes(
        &entries(&[(PAGE, "src/app/docs/[[...slug]]/page.rs", &[])]),
        &[],
        &config(),
    );
    assert!(out.contains(".route(\"/docs\","), "{out}");
    assert!(out.contains(".route(\"/docs/{*slug}\","), "{out}");
}

// ----------------------------------------------------------------- layouts

#[test]
fn a_page_is_wrapped_in_the_layouts_above_it() {
    let out = gen_routes(&app(), &[], &config());
    // The outermost layout owns the document, so it is applied by its own
    // call — the one that stamps the identity the client navigates by.
    assert!(
        out.contains("::rahti::apply_root_layout(l_root::layout, \"layout_"),
        "{out}"
    );
    assert!(
        out.contains("::rahti::apply_layout(l_marketing::layout, res)"),
        "{out}"
    );
}

/// A later `.layer` sits further out, so the deepest layout is emitted first
/// and the response climbs out through the root.
#[test]
fn layouts_are_layered_innermost_first() {
    let out = gen_routes(
        &entries(&[
            (LAYOUT, "src/app/layout.rs", &[]),
            (LAYOUT, "src/app/(marketing)/layout.rs", &[]),
            (PAGE, "src/app/(marketing)/adds/page.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert!(
        at(&out, "l_marketing::layout") < at(&out, "l_root::layout"),
        "{out}"
    );
}

/// Routes with the same wrapping share a sub-router, because a layout is a
/// layer and a layer covers a whole router.
#[test]
fn routes_sharing_a_chain_share_a_router() {
    let out = gen_routes(
        &entries(&[
            (LAYOUT, "src/app/layout.rs", &[]),
            (PAGE, "src/app/page.rs", &[]),
            (PAGE, "src/app/users/page.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert_eq!(out.matches("let group_").count(), 1, "{out}");
    assert_eq!(
        out.matches("apply_root_layout(l_root::layout").count(),
        1,
        "{out}"
    );
}

/// `route.rs` answers with JSON or a redirect, so it inherits no layout —
/// Next.js does not wrap those either.
#[test]
fn an_api_route_inherits_no_layout() {
    let out = gen_routes(
        &entries(&[
            (LAYOUT, "src/app/layout.rs", &[]),
            (PAGE, "src/app/page.rs", &[]),
            (ROUTE, "src/app/api/route.rs", &["get"]),
        ]),
        &[],
        &config(),
    );
    let group = group_for(&out, "/api");
    assert!(!group.contains("apply_layout"), "{group}");
}

// -------------------------------------------------------- error boundaries

/// The nearest boundary above a page wins.
#[test]
fn the_deepest_boundary_catches_a_page() {
    let out = gen_routes(&app(), &[], &config());
    let group = group_for(&out, "/customers");
    assert!(group.contains("apply_error(e_customers::error"), "{group}");
    assert!(!group.contains("apply_error(e_root::error"), "{group}");
}

/// A panic has to become a response the boundary above can render, rather
/// than killing the connection — so it is caught closest to the handler.
#[test]
fn panics_are_caught_below_the_boundary() {
    let out = gen_routes(&app(), &[], &config());
    let group = group_for(&out, "/customers/{id}");
    assert!(
        at(group, "CatchPanicLayer") < at(group, "apply_error"),
        "{group}"
    );
}

/// Layouts below the boundary belong to the subtree that failed, so the error
/// UI replaces them rather than sitting inside them — while the layouts above
/// still wrap it.
#[test]
fn the_error_ui_replaces_the_layouts_below_its_boundary() {
    let out = gen_routes(
        &entries(&[
            (LAYOUT, "src/app/layout.rs", &[]),
            (ERROR, "src/app/layout_test_boundary.rs", &[]),
            (LAYOUT, "src/app/shop/layout.rs", &[]),
            (ERROR, "src/app/error.rs", &[]),
            (PAGE, "src/app/shop/page.rs", &[]),
        ]),
        &[],
        &config(),
    );
    // The root boundary sits above the shop layout, so the shop layout is
    // applied below it and the root layout above it.
    assert!(
        at(&out, "apply_layout(l_shop::layout") < at(&out, "apply_error("),
        "{out}"
    );
    assert!(
        at(&out, "apply_error(") < at(&out, "apply_root_layout(l_root::layout"),
        "{out}"
    );
}

/// A root layout is applied outside the boundary even when the boundary sits
/// above it in the tree: an error page is still a page, and without the
/// document it would come back as a bare fragment.
#[test]
fn the_error_ui_stays_inside_its_root_layout() {
    let out = gen_routes(
        &as_root(
            entries(&[
                (ERROR, "src/app/error.rs", &[]),
                (LAYOUT, "src/app/(portal)/layout.rs", &[]),
                (PAGE, "src/app/(portal)/page.rs", &[]),
            ]),
            "src/app/(portal)/layout.rs",
        ),
        &[],
        &config(),
    );
    assert!(
        at(&out, "apply_error(") < at(&out, "apply_root_layout(l_portal::layout"),
        "{out}"
    );
}

// ------------------------------------------------------ multiple root layouts

/// Every page's document is named in the response, and two shells never share
/// a name: that pair is what tells the client a navigation cannot be a swap.
#[test]
fn each_root_layout_is_applied_under_its_own_name() {
    let out = gen_routes(
        &as_root(
            entries(&[
                (LAYOUT, "src/app/layout.rs", &[]),
                (PAGE, "src/app/page.rs", &[]),
                (LAYOUT, "src/app/(portal)/layout.rs", &[]),
                (PAGE, "src/app/(portal)/portal/page.rs", &[]),
            ]),
            "src/app/(portal)/layout.rs",
        ),
        &[],
        &config(),
    );

    let main = route::component_id(Kind::Layout, "layout.rs");
    let portal = route::component_id(Kind::Layout, "(portal)/layout.rs");
    assert_ne!(main, portal);
    assert!(
        out.contains(&format!("apply_root_layout(l_root::layout, \"{main}\"")),
        "{out}"
    );
    assert!(
        out.contains(&format!("apply_root_layout(l_portal::layout, \"{portal}\"")),
        "{out}"
    );
}

/// A layout that owns a document ends the inheritance. Applying `app/layout.rs`
/// above it would put a second `<html>` around the first.
#[test]
fn a_root_layout_is_not_wrapped_in_the_layout_above_it() {
    let out = gen_routes(
        &as_root(
            entries(&[
                (LAYOUT, "src/app/layout.rs", &[]),
                (LAYOUT, "src/app/(portal)/layout.rs", &[]),
                (PAGE, "src/app/(portal)/portal/page.rs", &[]),
            ]),
            "src/app/(portal)/layout.rs",
        ),
        &[],
        &config(),
    );
    // Nothing else is under `app/layout.rs`, so it is now inherited by no
    // page at all — module included.
    assert!(!out.contains("l_root"), "{out}");
    assert!(out.contains("apply_root_layout(l_portal::layout"), "{out}");
}

/// Next.js's shape: no `app/layout.rs`, and each top-level group brings its
/// own. The outermost layout of a chain is the root whether it says so or not.
#[test]
fn the_outermost_layout_is_the_root_without_a_marker() {
    let out = gen_routes(
        &entries(&[
            (LAYOUT, "src/app/(shop)/layout.rs", &[]),
            (PAGE, "src/app/(shop)/cart/page.rs", &[]),
            (LAYOUT, "src/app/(admin)/layout.rs", &[]),
            (PAGE, "src/app/(admin)/users/page.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert!(out.contains("apply_root_layout(l_shop::layout"), "{out}");
    assert!(out.contains("apply_root_layout(l_admin::layout"), "{out}");
    assert!(!out.contains("::rahti::apply_layout("), "{out}");
}

#[test]
fn a_page_without_a_boundary_gets_no_panic_layer() {
    let out = gen_routes(&entries(&[(PAGE, "src/app/page.rs", &[])]), &[], &config());
    assert!(!out.contains("CatchPanicLayer"), "{out}");
}

// ------------------------------------------------------------------- 404

#[test]
fn the_not_found_page_is_wrapped_and_served_as_a_fallback() {
    let out = gen_routes(&app(), &[], &config());
    assert!(out.contains("async fn not_found()"), "{out}");
    assert!(out.contains("axum::http::StatusCode::NOT_FOUND"), "{out}");
    assert!(
        out.contains("l_root::layout(nf_root::not_found().await)"),
        "{out}"
    );
    assert!(
        out.contains(".not_found_service(axum::routing::any(not_found))"),
        "{out}"
    );
}

/// The 404 renders below the static-file fallback, with no layer to carry the
/// document's identity — so the generated handler carries it instead, and a
/// navigation into a 404 is judged like any other.
#[test]
fn the_not_found_page_names_its_root_layout() {
    let out = gen_routes(&app(), &[], &config());
    let id = route::component_id(Kind::Layout, "layout.rs");
    assert!(
        out.contains(&format!("::rahti::root_document(\"{id}\", l_root::layout(")),
        "{out}"
    );
}

#[test]
fn without_a_not_found_file_the_static_service_answers() {
    let out = gen_routes(&entries(&[(PAGE, "src/app/page.rs", &[])]), &[], &config());
    assert!(!out.contains("not_found_service"), "{out}");
    assert!(out.contains("fallback_service"), "{out}");
}

/// A 404 file below the app root answers nothing: only the root one is wired.
#[test]
fn only_the_root_not_found_is_wired() {
    let out = gen_routes(
        &entries(&[
            (PAGE, "src/app/page.rs", &[]),
            (NOT_FOUND, "src/app/deep/not-found.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert!(!out.contains("not_found_service"), "{out}");
}

// ---------------------------------------------------------------- loading

fn with_loading() -> Vec<Entry> {
    entries(&[
        (LAYOUT, "src/app/layout.rs", &[]),
        (PAGE, "src/app/page.rs", &[]),
        (LOADING, "src/app/loading.rs", &[]),
        (PAGE, "src/app/slow/page.rs", &[]),
        (LOADING, "src/app/slow/loading.rs", &[]),
    ])
}

/// Every region is installed once, before the router is assembled: a loading
/// region takes no arguments, so it cannot differ between two requests.
#[test]
fn every_loading_region_is_installed_with_the_url_it_covers() {
    let out = gen_routes(&with_loading(), &[], &config());
    assert!(out.contains("::rahti::install_loading(&["), "{out}");
    assert!(
        out.contains("(\"/\", ld_root::loading as fn() -> ::rahti::Html)"),
        "{out}"
    );
    assert!(
        out.contains("(\"/slow\", ld_slow::loading as fn() -> ::rahti::Html)"),
        "{out}"
    );
}

#[test]
fn a_loading_region_gets_a_module_of_its_own() {
    let out = gen_routes(&with_loading(), &[], &config());
    assert!(out.contains("#[path = \"app/slow/loading.rs\"]"), "{out}");
}

/// A region belongs to no route group. It is picked in the browser, from the
/// path the reader is on, so the wrapping a page inherits has nothing to say
/// about it — and it is never a layer.
#[test]
fn a_loading_region_is_not_a_layer() {
    let out = gen_routes(&with_loading(), &[], &config());
    assert!(!out.contains("ld_root::loading,"), "{out}");
    assert!(
        !out.contains("map_response(|res| ::rahti::apply_loading"),
        "{out}"
    );
}

#[test]
fn the_install_call_comes_before_the_routes() {
    let out = gen_routes(&with_loading(), &[], &config());
    assert!(
        at(&out, "::rahti::install_loading") < at(&out, "let group_0"),
        "{out}"
    );
}

#[test]
fn an_app_without_a_loading_file_installs_nothing() {
    let out = gen_routes(&app(), &[], &config());
    assert!(!out.contains("install_loading"), "{out}");
}

/// A group adds no URL segment, so the region inside it answers for the
/// group's parent path.
#[test]
fn a_region_inside_a_group_covers_the_groups_parent_path() {
    let out = gen_routes(
        &entries(&[
            (PAGE, "src/app/(marketing)/adds/page.rs", &[]),
            (LOADING, "src/app/(marketing)/loading.rs", &[]),
        ]),
        &[],
        &config(),
    );
    assert!(out.contains("(\"/\", ld_marketing::loading"), "{out}");
}

#[test]
fn the_manifest_records_what_each_region_covers() {
    let out = gen_manifest(&with_loading(), &[], &[], None, &config());
    assert!(
        out.contains("{ \"file\": \"src/app/slow/loading.rs\", \"component\": "),
        "{out}"
    );
    assert!(out.contains("\"covers\": \"/slow\""), "{out}");
}

// -------------------------------------------------------------- the router

#[test]
fn every_group_is_merged_into_one_router() {
    let out = gen_routes(&app(), &[], &config());
    let groups = out.matches("let group_").count();
    assert!(groups > 1, "this app has several distinct wrappings: {out}");
    assert_eq!(out.matches(".merge(group_").count(), groups, "{out}");
}

/// Empty outside dev mode, so wiring it in unconditionally costs a release
/// build nothing.
#[test]
fn the_dev_routes_are_always_wired() {
    let out = gen_routes(&app(), &[], &config());
    assert!(out.contains(".merge(::rahti::dev_routes("), "{out}");
}

#[test]
fn anything_unclaimed_falls_through_to_public() {
    let out = gen_routes(&app(), &[], &config());
    assert!(
        out.contains("ServeDir::new(::rahti::public_dir(PUBLIC_DIR))"),
        "{out}"
    );
}

/// The file is checked in and read by people, so it says where it came from.
#[test]
fn the_output_says_it_is_generated() {
    let out = gen_routes(&app(), &[], &config());
    assert!(out.starts_with("// @generated by rahti-build"), "{out}");
    assert!(out.contains("src/app/"), "{out}");
    // The doc comment above `router` lists what answers what.
    assert!(
        out.contains("/// - `/users` [GET] — src/app/users/page.rs"),
        "{out}"
    );
    assert!(out.contains("[GET, POST]"), "{out}");
    assert!(out.contains("layouts: src/app/layout.rs"), "{out}");
    assert!(out.contains("errors: src/app/customers/error.rs"), "{out}");
}

// ------------------------------------------------------------------ manifest

#[test]
fn the_manifest_lists_every_url() {
    let out = gen_manifest(&app(), &[], &[], None, &config());
    assert!(out.contains("\"url\": \"/users\""), "{out}");
    assert!(out.contains("\"url\": \"/customers/{id}\""), "{out}");
    assert!(out.contains("\"methods\": [\"GET\", \"POST\"]"), "{out}");
    assert!(out.contains("\"appDir\": \"src/app\""), "{out}");
}

/// The manifest describes the markup that ships, so the name it reports is
/// the one the macro stamps.
#[test]
fn the_manifest_reports_the_component_names() {
    let entries = app();
    let out = gen_manifest(&entries, &[], &[], None, &config());
    let page = entries
        .iter()
        .find(|e| e.file == "src/app/users/page.rs")
        .unwrap();
    assert!(
        out.contains(&format!(
            "\"component\": \"{}\"",
            page.component.as_deref().unwrap()
        )),
        "{out}"
    );
    // A `route.rs` renders none.
    assert!(out.contains("\"component\": null"), "{out}");
}

#[test]
fn the_manifest_says_what_each_layout_wraps() {
    let out = gen_manifest(&app(), &[], &[], None, &config());
    let layouts = out
        .split("\"layouts\"")
        .nth(1)
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        layouts.contains("src/app/(marketing)/layout.rs"),
        "{layouts}"
    );
    assert!(layouts.contains("\"wraps\": [\"/adds\"]"), "{layouts}");
}

#[test]
fn the_manifest_says_what_each_boundary_catches() {
    let out = gen_manifest(&app(), &[], &[], None, &config());
    let errors = out
        .split("\"errors\"")
        .nth(1)
        .unwrap_or_else(|| panic!("{out}"));
    assert!(errors.contains("src/app/customers/error.rs"), "{errors}");
    assert!(errors.contains("\"/customers\""), "{errors}");
    assert!(errors.contains("\"/customers/{id}\""), "{errors}");
}

#[test]
fn the_manifest_lists_the_component_files() {
    let components = vec![
        "src/components/button.rs".to_string(),
        "src/components/user_card.rs".to_string(),
    ];
    let out = gen_manifest(&app(), &components, &[], None, &config());
    assert!(
        out.contains("{ \"file\": \"src/components/user_card.rs\", \"rpcs\": [] }"),
        "{out}"
    );
}

/// Tooling reads this, so it has to parse: every list is comma-separated with
/// nothing trailing, and the braces balance.
#[test]
fn the_manifest_is_well_formed() {
    let out = gen_manifest(
        &app(),
        &["src/components/button.rs".to_string()],
        &[],
        None,
        &config(),
    );
    assert!(
        !out.contains(",\n  ]"),
        "a trailing comma before a close: {out}"
    );
    assert!(
        !out.contains(",\n]"),
        "a trailing comma before the end: {out}"
    );
    assert_eq!(out.matches('{').count(), out.matches('}').count(), "{out}");
    assert_eq!(out.matches('[').count(), out.matches(']').count(), "{out}");
    assert!(out.trim_end().ends_with('}'), "{out}");
}

#[test]
fn an_empty_app_still_produces_a_manifest() {
    let out = gen_manifest(&[], &[], &[], None, &config());
    assert!(out.contains("\"routes\": ["), "{out}");
    assert_eq!(out.matches('{').count(), out.matches('}').count(), "{out}");
}

// --------------------------------------------------------------------- rpc

/// The dispatcher is the whole of the runtime lookup: a `match` on the name
/// the client sent, written out here so the compiler resolves it.
#[test]
fn a_page_with_rpcs_gets_a_dispatcher() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet", "divide"]);
    let out = gen_routes(&entries, &[], &config());
    assert!(
        out.contains("async fn rpc_p_users(req: axum::extract::Request)"),
        "{out}"
    );
    assert!(
        out.contains("\"greet\" => p_users::__rahti_rpc_greet(req).await"),
        "{out}"
    );
    assert!(
        out.contains("\"divide\" => p_users::__rahti_rpc_divide(req).await"),
        "{out}"
    );
    assert!(
        out.contains("other => ::rahti::rpc_unknown(other)"),
        "{out}"
    );
}

/// POST on the page's own URL, which is where the client sends it.
#[test]
fn the_rpcs_answer_the_pages_own_url() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet"]);
    let out = gen_routes(&entries, &[], &config());
    assert!(
        out.contains(".route(\"/users\", axum::routing::post(rpc_p_users))"),
        "{out}"
    );
    // And the page is still a GET.
    assert!(
        out.contains(".route(\"/users\", axum::routing::get(p_users::page))"),
        "{out}"
    );
}

/// A layout would wrap the JSON in a document, and an error boundary would
/// replace it with one. The rpc router carries neither.
#[test]
fn the_rpcs_are_not_wrapped_in_the_pages_layers() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet"]);
    let out = gen_routes(&entries, &[], &config());
    let rpc = out
        .split("let rpc = Router::new()")
        .nth(1)
        .unwrap_or_else(|| panic!("{out}"));
    let rpc = rpc.split(";").next().unwrap();
    assert!(!rpc.contains("apply_layout"), "{rpc}");
    assert!(!rpc.contains("apply_error"), "{rpc}");
    // A panic still has to come back as JSON.
    assert!(rpc.contains("::rahti::rpc_panic_response"), "{rpc}");
}

/// The token the client sends back is set on the page it is called from, so
/// the layer sits outside everything.
#[test]
fn an_app_with_rpcs_issues_csrf_tokens() {
    let out = gen_routes(
        &with_rpcs(app(), "src/app/users/page.rs", &["greet"]),
        &[],
        &config(),
    );
    assert!(
        out.contains(".layer(axum::middleware::from_fn(::rahti::csrf))"),
        "{out}"
    );
}

/// The auth layer goes over every page group, so a private route is refused
/// before its handler runs — and over the rpc router, so `#[rpc(auth)]` has a
/// session to check and `auth::sign_in` has somewhere to write its cookie.
#[test]
fn every_group_and_the_rpc_router_carry_the_auth_layer() {
    let out = gen_routes(
        &with_rpcs(app(), "src/app/users/page.rs", &["greet"]),
        &[],
        &config(),
    );

    let layer = ".layer(axum::middleware::from_fn(::rahti::auth_guard))";
    let groups = out.matches("let group_").count();
    // One per group, plus the rpc router.
    assert_eq!(out.matches(layer).count(), groups + 1, "{out}");

    // Outermost of a group's layers, so a redirect is not wrapped in the
    // document the page would have rendered in.
    let group = out
        .split("let group_0 = Router::new()")
        .nth(1)
        .and_then(|rest| rest.split(";").next())
        .unwrap_or_else(|| panic!("{out}"));
    assert!(group.trim_end().ends_with(layer.trim()), "{group}");
}

/// Unlike the CSRF layer, which exists only when there is an rpc to protect:
/// an app with no rpcs can still have a page behind a login.
#[test]
fn an_app_without_rpcs_still_carries_the_auth_layer() {
    let out = gen_routes(&app(), &[], &config());
    assert!(
        out.contains(".layer(axum::middleware::from_fn(::rahti::auth_guard))"),
        "{out}"
    );
}

/// Nothing to protect, nothing to pay: an app without an `#[rpc]` gets no
/// dispatcher, no extra router and no cookie.
#[test]
fn an_app_without_rpcs_carries_none_of_it() {
    let out = gen_routes(&app(), &[], &config());
    assert!(!out.contains("rpc"), "{out}");
}

#[test]
fn the_doc_comment_names_the_rpcs_a_page_answers() {
    let out = gen_routes(
        &with_rpcs(app(), "src/app/users/page.rs", &["greet"]),
        &[],
        &config(),
    );
    assert!(out.contains("rpc: greet"), "{out}");
}

/// An optional catch-all answers two URLs, and both of them are the page.
#[test]
fn every_url_of_a_page_answers_its_rpcs() {
    let entries = with_rpcs(
        entries(&[(PAGE, "src/app/docs/[[...slug]]/page.rs", &[])]),
        "src/app/docs/[[...slug]]/page.rs",
        &["search"],
    );
    let out = gen_routes(&entries, &[], &config());
    assert!(
        out.contains(".route(\"/docs\", axum::routing::post(rpc_p_docs_slug))"),
        "{out}"
    );
    assert!(
        out.contains(".route(\"/docs/{*slug}\", axum::routing::post(rpc_p_docs_slug))"),
        "{out}"
    );
}

#[test]
fn the_manifest_lists_the_rpcs_of_a_page() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet", "divide"]);
    let out = gen_manifest(&entries, &[], &[], None, &config());
    let line = out
        .lines()
        .find(|line| line.contains("src/app/users/page.rs"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(line.contains("\"rpcs\": [\"greet\", \"divide\"]"), "{line}");
    assert!(line.contains("\"methods\": [\"GET\", \"POST\"]"), "{line}");
}

/// An upload is larger than axum's default body limit, and a limit has to be
/// raised where the body arrives rather than where it is read.
#[test]
fn the_rpc_router_accepts_an_upload_sized_body() {
    let out = gen_routes(
        &with_rpcs(app(), "src/app/users/page.rs", &["receive"]),
        &[],
        &config(),
    );
    assert!(
        out.contains(".layer(axum::extract::DefaultBodyLimit::max(::rahti::max_upload()))"),
        "{out}"
    );
}

// -------------------------------------------------------- rpc in a component

/// The rpcs a `src/components/` file declares.
fn component_rpcs(rpcs: &[(&str, &str)]) -> Vec<Rpc> {
    rpcs.iter()
        .map(|(module, name)| Rpc {
            name: (*name).to_string(),
            module: (*module).to_string(),
            file: format!("src/components/{}.rs", module.replace("::", "/")),
        })
        .collect()
}

/// A component has no URL of its own, so every page falls through to one
/// dispatcher — which asks the link-time registry rather than a generated
/// `match`, because a component library's rpcs are never scanned by this
/// build and could not appear in one.
#[test]
fn a_component_rpc_gets_a_dispatcher_of_its_own() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );

    assert!(
        out.contains("async fn component_rpc(name: &str, req: axum::extract::Request)"),
        "{out}"
    );
    assert!(
        out.contains("::rahti::dispatch_registered(name, PAGE_RPCS, req).await"),
        "{out}"
    );
}

/// A page's rpcs are in the registry too, so the fallback is handed the list
/// to refuse: without it any page could answer any other page's rpc.
#[test]
fn the_page_rpcs_are_withheld_from_the_fallback() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet", "hello"]);
    let out = gen_routes(
        &entries,
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );

    assert!(out.contains("const PAGE_RPCS: &[&str] = &["), "{out}");
    for name in ["greet", "hello"] {
        assert!(
            out.contains(&format!("    \"{name}\",")),
            "{name} missing from PAGE_RPCS:\n{out}"
        );
    }
    assert!(
        !out.contains("    \"like\","),
        "a component rpc belongs to no URL, so it is not withheld:\n{out}"
    );
}

/// A binary can only be checked for clashing names once everything is linked
/// into it, which is the first thing the router does.
#[test]
fn the_router_checks_the_rpc_names_at_startup() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );
    assert!(out.contains("::rahti::check_rpc_names();"), "{out}");
}

/// A library's rpcs are invisible to the scan, so its mere presence is reason
/// enough to generate the fallback — otherwise the page never answers POST
/// and the rpc is unreachable however correctly the library declared it.
#[test]
fn a_library_alone_generates_the_fallback() {
    let mut config = config();
    config.has_libraries = true;

    let out = gen_routes(&app(), &[], &config);
    assert!(out.contains("async fn component_rpc("), "{out}");
    assert!(out.contains("::rahti::dispatch_registered("), "{out}");
}

/// The client posts to the page it is on, and the page cannot know which
/// components it rendered — so a name it does not have falls through rather
/// than being refused.
#[test]
fn a_page_falls_through_to_the_components() {
    let entries = with_rpcs(app(), "src/app/users/page.rs", &["greet"]);
    let out = gen_routes(
        &entries,
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );

    let dispatcher = out
        .split("async fn rpc_p_users")
        .nth(1)
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        dispatcher.contains("\"greet\" => p_users::__rahti_rpc_greet(req).await"),
        "{out}"
    );
    assert!(
        dispatcher.contains("_ => component_rpc(&name, req).await"),
        "{out}"
    );
}

/// A page with no `#[rpc]` of its own still renders components, so its URL
/// answers POST with nothing but the fallthrough.
#[test]
fn a_page_without_rpcs_still_answers_post_for_them() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );

    assert!(
        out.contains("async fn rpc_components(req: axum::extract::Request)"),
        "{out}"
    );
    assert!(
        out.contains(".route(\"/users\", axum::routing::post(rpc_components))"),
        "{out}"
    );
    assert!(
        out.contains(".route(\"/\", axum::routing::post(rpc_components))"),
        "{out}"
    );
}

/// A `route.rs` answers its own methods and renders no component, so it is
/// left alone.
#[test]
fn a_route_handler_is_not_given_a_component_dispatcher() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );
    assert!(
        !out.contains(".route(\"/api/health\", axum::routing::post(rpc_components))"),
        "{out}"
    );
}

/// The cookie the client reads back is what makes the call possible, and a
/// component rpc needs it as much as a page's.
#[test]
fn a_component_rpc_alone_still_issues_csrf_tokens() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );
    assert!(
        out.contains(".layer(axum::middleware::from_fn(::rahti::csrf))"),
        "{out}"
    );
    assert!(out.contains("::rahti::rpc_panic_response"), "{out}");
}

#[test]
fn the_doc_comment_names_the_component_rpcs() {
    let out = gen_routes(
        &app(),
        &component_rpcs(&[("like_button", "like")]),
        &config(),
    );
    assert!(out.contains("also answers the component rpcs"), "{out}");
    assert!(
        out.contains("/// - `/users` [GET, POST] — src/app/users/page.rs"),
        "{out}"
    );
}

#[test]
fn the_manifest_lists_the_rpcs_of_a_component() {
    let out = gen_manifest(
        &app(),
        &["src/components/like_button.rs".to_string()],
        &component_rpcs(&[("like_button", "like")]),
        None,
        &config(),
    );

    assert!(
        out.contains(
            "{ \"file\": \"src/components/like_button.rs\", \"rpcs\": [{ \"name\": \"like\", \"module\": \"like_button\" }] }"
        ),
        "{out}"
    );
    let page = out
        .lines()
        .find(|line| line.contains("src/app/users/page.rs"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(page.contains("\"methods\": [\"GET\", \"POST\"]"), "{page}");
}

// ----------------------------------------------------------------- sockets

/// The endpoint is merged only when a `#[socket]` exists to answer on it —
/// an app without one must never reference `rahti::ws`, because the module
/// only exists behind the `ws` feature.
#[test]
fn sockets_merge_the_endpoint() {
    let out = routes_rs(
        &app(),
        &[],
        &sockets(&[("chat", "src/app/page.rs")]),
        &config(),
    );
    assert!(out.contains(".merge(::rahti::ws::routes())"), "{out}");
    // The hint sits beside the line the missing feature would break.
    assert!(out.contains("features = [\"ws\"]"), "{out}");
    // And the doc comment names what connects where.
    assert!(out.contains("/// - `chat` — src/app/page.rs"), "{out}");
}

#[test]
fn no_sockets_means_no_endpoint() {
    let out = gen_routes(&app(), &[], &config());
    assert!(!out.contains("rahti::ws"), "{out}");
}

#[test]
fn the_manifest_lists_the_sockets() {
    let out = manifest_json(
        &app(),
        &[],
        &[],
        &sockets(&[
            ("chat", "src/app/page.rs"),
            ("feed", "src/components/ticker.rs"),
        ]),
        None,
        &config(),
    );
    assert!(
        out.contains("{ \"name\": \"chat\", \"file\": \"src/app/page.rs\" },"),
        "{out}"
    );
    assert!(
        out.contains("{ \"name\": \"feed\", \"file\": \"src/components/ticker.rs\" }"),
        "{out}"
    );
}

#[test]
fn an_app_without_sockets_still_says_so() {
    let out = gen_manifest(&app(), &[], &[], None, &config());
    assert!(out.contains("\"sockets\": [\n  ],"), "{out}");
}
