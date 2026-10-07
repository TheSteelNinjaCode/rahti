//! Walking `app/`: what is found, what is watched, and what is refused.

use super::*;
use crate::tree::Tree;

/// A walked tree, with entries looked up by their path below its root.
struct Scanned {
    root: String,
    scan: Scan,
}

fn walk_tree(tree: &Tree) -> Scanned {
    Scanned {
        root: tree.path().to_string_lossy().replace('\\', "/"),
        scan: scan(tree.path()),
    }
}

impl Scanned {
    /// The entry for `relative`, a path below the app root.
    fn entry(&self, relative: &str) -> &Entry {
        let wanted = format!("{}/{relative}", self.root);
        self.scan
            .entries
            .iter()
            .find(|e| e.file == wanted)
            .unwrap_or_else(|| {
                panic!(
                    "no entry for {relative}; found {:?}",
                    self.scan
                        .entries
                        .iter()
                        .map(|e| &e.file)
                        .collect::<Vec<_>>()
                )
            })
    }

    fn urls(&self, relative: &str) -> Vec<String> {
        self.entry(relative).urls.clone()
    }

    fn entries(&self) -> &[Entry] {
        &self.scan.entries
    }

    fn watched(&self) -> &[String] {
        &self.scan.watched
    }
}

// ------------------------------------------------------------------ finding

#[test]
fn every_convention_file_is_found() {
    let tree = Tree::new("finds");
    tree.page("page.rs")
        .layout("layout.rs")
        .error("error.rs")
        .file(
            "not-found.rs",
            "pub async fn not_found() -> Html { todo!() }",
        )
        .loading("loading.rs")
        .file("api/health/route.rs", "pub async fn get() {}");

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entries().len(), 6);
    assert_eq!(scanned.entry("page.rs").kind, Kind::Page);
    assert_eq!(scanned.entry("layout.rs").kind, Kind::Layout);
    assert_eq!(scanned.entry("error.rs").kind, Kind::Error);
    assert_eq!(scanned.entry("not-found.rs").kind, Kind::NotFound);
    assert_eq!(scanned.entry("loading.rs").kind, Kind::Loading);
    assert_eq!(scanned.entry("api/health/route.rs").kind, Kind::Route);
}

#[test]
fn ordinary_code_beside_a_page_is_ignored() {
    let tree = Tree::new("ignores");
    tree.page("page.rs")
        .file("helpers.rs", "pub fn help() {}")
        .file("README.md", "hello");

    assert_eq!(walk_tree(&tree).entries().len(), 1);
}

/// `_folder` is colocated code — never routed, and never walked.
#[test]
fn an_underscored_directory_is_private() {
    let tree = Tree::new("private");
    tree.page("page.rs").page("_lib/page.rs");

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entries().len(), 1);
    assert!(!scanned.watched().iter().any(|w| w.contains("_lib")));
}

/// The filesystem enumerates in whatever order it likes; the scan must not.
#[test]
fn the_scan_is_ordered() {
    let tree = Tree::new("ordered");
    tree.page("zebra/page.rs")
        .page("alpha/page.rs")
        .page("page.rs");

    let scanned = walk_tree(&tree);
    let files: Vec<&str> = scanned.entries().iter().map(|e| e.file.as_str()).collect();
    let mut sorted = files.clone();
    sorted.sort();
    assert_eq!(files, sorted);
}

// --------------------------------------------------------------------- urls

#[test]
fn a_page_answers_the_url_its_directory_spells() {
    let tree = Tree::new("urls");
    tree.page("page.rs")
        .page("users/page.rs")
        .page("customers/[id]/page.rs")
        .page("posts/[...id]/page.rs")
        .page("docs/[[...slug]]/page.rs")
        .page("(marketing)/adds/page.rs");

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.urls("page.rs"), vec!["/"]);
    assert_eq!(scanned.urls("users/page.rs"), vec!["/users"]);
    assert_eq!(
        scanned.urls("customers/[id]/page.rs"),
        vec!["/customers/{id}"]
    );
    assert_eq!(scanned.urls("posts/[...id]/page.rs"), vec!["/posts/{*id}"]);
    assert_eq!(
        scanned.urls("docs/[[...slug]]/page.rs"),
        vec!["/docs", "/docs/{*slug}"]
    );
    assert_eq!(scanned.urls("(marketing)/adds/page.rs"), vec!["/adds"]);
}

/// A layout wraps pages, an error boundary catches them; neither is reachable
/// on its own, so neither claims a URL.
#[test]
fn layouts_and_boundaries_claim_no_url() {
    let tree = Tree::new("nourls");
    tree.page("page.rs").layout("layout.rs").error("error.rs");

    let scanned = walk_tree(&tree);
    assert!(scanned.entry("layout.rs").urls.is_empty());
    assert!(scanned.entry("error.rs").urls.is_empty());
}

#[test]
fn pages_and_layouts_report_static_metadata() {
    let tree = Tree::new("metadata");
    tree.file(
        "layout.rs",
        "pub fn metadata() -> Metadata { todo!() }\npub fn layout(children: Html) -> Html { children }",
    )
    .file(
        "page.rs",
        "pub fn metadata() -> Metadata { todo!() }\npub async fn page() -> Html { todo!() }",
    )
    .file(
        "plain/page.rs",
        "/// `pub fn metadata()` is only an example.\npub async fn page() -> Html { todo!() }",
    );

    let scanned = walk_tree(&tree);
    assert!(scanned.entry("layout.rs").metadata);
    assert!(scanned.entry("page.rs").metadata);
    assert!(!scanned.entry("plain/page.rs").metadata);
}

#[test]
fn a_dynamic_page_reports_its_static_paths_provider() {
    let tree = Tree::new("static-paths");
    tree.file(
        "posts/[id]/page.rs",
        "pub async fn static_paths() -> Vec<&'static str> { vec![\"/posts/1\"] }\npub async fn page() -> Html { todo!() }",
    );
    assert!(walk_tree(&tree).entry("posts/[id]/page.rs").static_paths);
}

#[test]
#[should_panic(expected = "not a dynamic page")]
fn a_static_page_does_not_take_static_paths() {
    let tree = Tree::new("unneeded-static-paths");
    tree.file(
        "about/page.rs",
        "pub async fn static_paths() -> Vec<&'static str> { vec![] }\npub async fn page() -> Html { todo!() }",
    );
    scan(tree.path());
}

#[test]
fn a_page_can_declare_its_cache_policy() {
    let tree = Tree::new("page-cache");
    tree.file(
        "news/page.rs",
        "pub fn cache() -> PageCache { PageCache::seconds(60) }\npub async fn page() -> Html { todo!() }",
    );
    assert!(walk_tree(&tree).entry("news/page.rs").page_cache);
}

#[test]
#[should_panic(expected = "page caching belongs only")]
fn a_layout_cannot_declare_a_page_cache() {
    let tree = Tree::new("layout-cache");
    tree.file(
        "layout.rs",
        "pub fn cache() -> PageCache { PageCache::seconds(60) }\npub fn layout(children: Html) -> Html { children }",
    )
    .file("page.rs", "pub async fn page() -> Html { todo!() }");
    scan(tree.path());
}

// ------------------------------------------------------------------ methods

#[test]
fn a_route_reports_the_methods_it_exports() {
    let tree = Tree::new("methods");
    tree.file(
        "api/route.rs",
        "pub async fn get() {}\npub async fn post() {}\npub async fn delete<T>() {}\npub async fn options() {}",
    );

    assert_eq!(
        walk_tree(&tree).entry("api/route.rs").methods,
        vec!["get", "post", "delete", "options"]
    );
}

/// A `route.rs` with nothing to call is a file the generated router would
/// fail to compile against.
#[test]
#[should_panic(expected = "exports no handlers")]
fn a_route_without_handlers_is_refused() {
    let tree = Tree::new("nomethods");
    tree.file("api/route.rs", "pub async fn handler() {}");
    scan(tree.path());
}

// ------------------------------------------------------------------ exports

#[test]
#[should_panic(expected = "exports no layout")]
fn a_layout_without_its_function_is_refused() {
    let tree = Tree::new("nolayout");
    tree.page("page.rs")
        .file("layout.rs", "// nothing here yet");
    scan(tree.path());
}

#[test]
#[should_panic(expected = "exports no error")]
fn an_error_boundary_without_its_function_is_refused() {
    let tree = Tree::new("noerror");
    tree.page("page.rs").file("error.rs", "// nothing here yet");
    scan(tree.path());
}

#[test]
#[should_panic(expected = "exports no loading")]
fn a_loading_region_without_its_function_is_refused() {
    let tree = Tree::new("noloading");
    tree.page("page.rs")
        .file("loading.rs", "// nothing here yet");
    scan(tree.path());
}

// ------------------------------------------------------------------ loading

/// The browser walks a real path up one literal segment at a time, so
/// `/customers/{id}` is a string it never looks for. A region written there
/// would ship and never match.
#[test]
#[should_panic(expected = "matched by literal path")]
fn a_loading_region_under_a_dynamic_segment_is_refused() {
    let tree = Tree::new("dynloading");
    tree.page("customers/[id]/page.rs")
        .loading("customers/[id]/loading.rs");
    scan(tree.path());
}

#[test]
#[should_panic(expected = "matched by literal path")]
fn a_loading_region_under_a_catch_all_is_refused() {
    let tree = Tree::new("catchloading");
    tree.page("posts/[...id]/page.rs")
        .loading("posts/[...id]/loading.rs");
    scan(tree.path());
}

/// A group contributes no URL segment, so a region inside one covers the
/// group's parent path — which is a literal path the browser can be on.
#[test]
fn a_loading_region_inside_a_route_group_is_kept() {
    let tree = Tree::new("grouploading");
    tree.page("(marketing)/adds/page.rs")
        .loading("(marketing)/loading.rs");
    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entry("(marketing)/loading.rs").kind, Kind::Loading);
}

// --------------------------------------------------------------- components

#[test]
fn every_file_that_renders_markup_is_named() {
    let tree = Tree::new("named");
    tree.page("customers/[id]/page.rs")
        .layout("layout.rs")
        .file("api/route.rs", "pub async fn get() {}");

    let scanned = walk_tree(&tree);
    assert_eq!(
        scanned.entry("customers/[id]/page.rs").component.as_deref(),
        Some(route::component_id(Kind::Page, "customers/[id]/page.rs").as_str())
    );
    assert_eq!(
        scanned.entry("layout.rs").component.as_deref(),
        Some(route::component_id(Kind::Layout, "layout.rs").as_str())
    );
    // A `route.rs` answers with JSON or a redirect — there is no markup to name.
    assert_eq!(scanned.entry("api/route.rs").component, None);
}

/// `html!` skips its own injection when the author wrote a name, so the
/// manifest has to report that name rather than the one it would have
/// derived — otherwise it describes markup that never ships.
#[test]
fn a_hand_written_name_is_what_gets_reported() {
    let tree = Tree::new("declared");
    tree.file(
        "layout.rs",
        "pub fn layout(children: Html) -> Html { html! {r##\"<template pp-component=\"rahti_main\"><slot /></template>\"##} }",
    )
    .page("page.rs");

    assert_eq!(
        walk_tree(&tree).entry("layout.rs").component.as_deref(),
        Some("rahti_main")
    );
}

/// A comment about the attribute is prose. The macro reads tokens and never
/// sees it; reading it here would report a name no markup carries — and a
/// layout's doc comment is exactly where the attribute gets explained.
#[test]
fn a_name_in_a_comment_is_not_a_declaration() {
    let tree = Tree::new("commented");
    tree.file(
        "layout.rs",
        "/// Rahti injects `<template pp-component=\"…\">` around the slot.\n\
         pub fn layout(children: Html) -> Html { html! {r##\"<html><body><slot /></body></html>\"##} }",
    )
    .page("page.rs");

    assert_eq!(
        walk_tree(&tree).entry("layout.rs").component.as_deref(),
        Some(route::component_id(Kind::Layout, "layout.rs").as_str())
    );
}

// ----------------------------------------------------------------- watching

/// Cargo compares a directory's own mtime, and deleting `app/(marketing)/x/`
/// bumps `(marketing)` while leaving `app` untouched. Watching only the root
/// would leave the generated router pointing at a deleted file.
#[test]
fn every_directory_and_file_is_watched() {
    let tree = Tree::new("watched");
    tree.page("page.rs").page("(marketing)/adds/page.rs");

    let scanned = walk_tree(&tree);
    for expected in [
        "/(marketing)",
        "/(marketing)/adds",
        "/(marketing)/adds/page.rs",
        "/page.rs",
    ] {
        assert!(
            scanned.watched().iter().any(|w| w.ends_with(expected)),
            "{expected} is not watched: {:?}",
            scanned.watched()
        );
    }
    assert!(scanned.watched().contains(&scanned.root));
}

// --------------------------------------------------------------- validation

#[test]
#[should_panic(expected = "cannot define both UI and an API handler")]
fn a_segment_cannot_be_both_ui_and_an_api() {
    let tree = Tree::new("both");
    tree.page("api/page.rs")
        .file("api/route.rs", "pub async fn get() {}");
    scan(tree.path());
}

/// A group and an optional catch-all both claim their parent path.
#[test]
#[should_panic(expected = "resolve to the same URL")]
fn two_files_cannot_answer_one_url() {
    let tree = Tree::new("dupurl");
    tree.page("page.rs").page("(marketing)/page.rs");
    scan(tree.path());
}

/// The macro derives a name from the source path with no view of the file's
/// siblings, so a clash can only be caught here.
#[test]
#[should_panic(expected = "would render `pp-component=")]
fn two_files_cannot_render_one_name() {
    let tree = Tree::new("dupname");
    tree.file(
        "a/layout.rs",
        "pub fn layout(children: Html) -> Html { html! {r##\"<div pp-component=\"same\"><slot /></div>\"##} }",
    )
    .file(
        "b/layout.rs",
        "pub fn layout(children: Html) -> Html { html! {r##\"<div pp-component=\"same\"><slot /></div>\"##} }",
    )
    .page("a/page.rs")
    .page("b/page.rs");
    scan(tree.path());
}

#[test]
#[should_panic(expected = "app directory not found")]
fn a_missing_app_directory_is_reported() {
    let tree = Tree::new("missing");
    scan(&tree.path().join("nowhere"));
}

/// An app with no pages yet still has to build.
#[test]
fn an_empty_app_directory_scans_to_nothing() {
    let tree = Tree::new("empty");
    let scanned = walk_tree(&tree);
    assert!(scanned.entries().is_empty());
    assert_eq!(
        scanned.watched().len(),
        1,
        "the root itself is still watched"
    );
}

// --------------------------------------------------------------------- rpc

/// The dispatcher is written from this list, so the order is the order the
/// arms are generated in and the names have to be the functions' own.
#[test]
fn a_page_records_its_rpc_functions() {
    let tree = Tree::new("rpcs");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[rpc]\n\
         pub async fn greet(name: String) -> Greeting { todo!() }\n\
         #[rpc]\n\
         async fn divide(a: f64, b: f64) -> Result<f64> { todo!() }\n",
    );

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entry("page.rs").rpcs, vec!["greet", "divide"]);
}

/// `#[rpc(auth)]` is the same rpc with a session check inside the shim, so the
/// dispatcher still has to know its name. A scan that only matched the bare
/// attribute would leave the function compiled and unreachable.
#[test]
fn a_guarded_rpc_is_recorded_like_any_other() {
    let tree = Tree::new("rpc-auth");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[rpc]\n\
         pub async fn read() -> Report { todo!() }\n\
         #[rpc(auth)]\n\
         pub async fn write(name: String) -> Report { todo!() }\n",
    );

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entry("page.rs").rpcs, vec!["read", "write"]);
    assert_eq!(scanned.entry("page.rs").auth_rpcs, vec!["write"]);
}

/// The arguments are the macro's to validate, against real tokens — but a line
/// that merely starts with the letters is not the attribute.
#[test]
fn something_that_only_looks_like_the_attribute_is_not_one() {
    let tree = Tree::new("rpc-lookalike");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[rpcs]\n\
         pub async fn nope() -> Report { todo!() }\n\
         #[rpc_auth]\n\
         pub async fn also_nope() -> Report { todo!() }\n",
    );
    assert!(walk_tree(&tree).entry("page.rs").rpcs.is_empty());
}

#[test]
fn a_page_without_rpcs_records_none() {
    let tree = Tree::new("no-rpcs");
    tree.page("page.rs");
    assert!(walk_tree(&tree).entry("page.rs").rpcs.is_empty());
}

/// The scan is a text scan, so the attribute counts where it is written as
/// one: on a line of its own, not inside the prose above a function.
#[test]
fn an_rpc_named_in_a_doc_comment_is_not_one() {
    let tree = Tree::new("rpc-doc");
    tree.file(
        "page.rs",
        "/// Mentions `#[rpc]` in passing.\n\
         pub async fn page() -> Html { todo!() }\n",
    );
    assert!(walk_tree(&tree).entry("page.rs").rpcs.is_empty());
}

/// The client POSTs to the URL the page is on, so a `route.rs` has no page
/// for the call to come from — the shim would be written and never reached.
#[test]
#[should_panic(expected = "under `app/` an `#[rpc]` belongs in a `page.rs`")]
fn an_rpc_outside_a_page_is_refused() {
    let tree = Tree::new("rpc-route");
    tree.file(
        "api/route.rs",
        "pub async fn get() -> String { todo!() }\n\
         #[rpc]\n\
         pub async fn greet(name: String) -> String { todo!() }\n",
    );
    walk_tree(&tree);
}

#[test]
#[should_panic(expected = "two rpcs named `greet`")]
fn two_rpcs_of_one_name_are_refused() {
    let tree = Tree::new("rpc-clash");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[rpc]\n\
         pub async fn greet(name: String) -> String { todo!() }\n\
         #[rpc]\n\
         pub async fn greet(other: String) -> String { todo!() }\n",
    );
    walk_tree(&tree);
}

#[test]
#[should_panic(expected = "with no function under it")]
fn an_rpc_over_nothing_is_refused() {
    let tree = Tree::new("rpc-dangling");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n#[rpc]\n",
    );
    walk_tree(&tree);
}

// ----------------------------------------------------------------- sockets

/// A socket is found the way an rpc is: as the attribute, on a line of its
/// own, in a `page.rs` — and `#[socket(auth)]` records the same name.
#[test]
fn a_page_records_its_socket_functions() {
    let tree = Tree::new("sockets");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[socket]\n\
         pub async fn chat(room: String, mut socket: Socket) { todo!() }\n\
         #[socket(auth)]\n\
         pub async fn feed(mut socket: Socket) { todo!() }\n",
    );

    let scanned = walk_tree(&tree);
    assert_eq!(scanned.entry("page.rs").sockets, vec!["chat", "feed"]);
    assert_eq!(scanned.entry("page.rs").auth_sockets, vec!["feed"]);
    assert!(scanned.entry("page.rs").rpcs.is_empty());
}

#[test]
#[should_panic(expected = "under `app/` a `#[socket]` belongs in a `page.rs`")]
fn a_socket_outside_a_page_is_refused() {
    let tree = Tree::new("socket-layout");
    tree.file(
        "layout.rs",
        "pub fn layout(children: Html) -> Html { todo!() }\n\
         #[socket]\n\
         pub async fn chat(mut socket: Socket) { todo!() }\n",
    );
    walk_tree(&tree);
}

#[test]
#[should_panic(expected = "two sockets named `chat`")]
fn two_sockets_of_one_name_in_one_file_are_refused() {
    let tree = Tree::new("socket-clash");
    tree.file(
        "page.rs",
        "pub async fn page() -> Html { todo!() }\n\
         #[socket]\n\
         pub async fn chat(mut socket: Socket) { todo!() }\n\
         #[socket]\n\
         pub async fn chat(other: String, mut socket: Socket) { todo!() }\n",
    );
    walk_tree(&tree);
}

// ------------------------------------------------------ multiple root layouts

/// `app/layout.rs` has nothing above it, so it owns the document whether or
/// not it says so. A layout below it does not, unless it declares itself one.
#[test]
fn only_the_app_root_layout_is_a_document_root_by_default() {
    let tree = Tree::new("root-implicit");
    tree.page("page.rs")
        .layout("layout.rs")
        .layout("(marketing)/layout.rs")
        .page("(marketing)/adds/page.rs");

    let scanned = walk_tree(&tree);
    assert!(scanned.entry("layout.rs").root);
    assert!(!scanned.entry("(marketing)/layout.rs").root);
}

/// The marker is what a second root layout is written with.
#[test]
fn a_layout_can_declare_itself_a_document_root() {
    let tree = Tree::new("root-declared");
    tree.page("page.rs").layout("layout.rs").file(
        "(portal)/layout.rs",
        "pub const ROOT_LAYOUT: bool = true;\n\
         pub fn layout(children: Html) -> Html { children }\n",
    );
    tree.page("(portal)/portal/page.rs");

    let scanned = walk_tree(&tree);
    let portal = scanned.entry("(portal)/portal/page.rs");

    assert!(scanned.entry("(portal)/layout.rs").root);
    // The chain stops there: `app/layout.rs` renders a document of its own,
    // and two documents cannot nest.
    assert_eq!(
        layout_chain(portal, scanned.entries())
            .iter()
            .map(|l| l.file.clone())
            .collect::<Vec<_>>(),
        vec![format!("{}/(portal)/layout.rs", scanned.root)]
    );
    assert_eq!(
        root_layout(scanned.entry("page.rs"), scanned.entries()).map(|l| l.file.clone()),
        Some(format!("{}/layout.rs", scanned.root))
    );
}

/// With no `app/layout.rs`, the outermost layout of each group is the root —
/// the shape Next.js asks for.
#[test]
fn top_level_group_layouts_are_roots_of_their_own() {
    let tree = Tree::new("root-groups");
    tree.layout("(shop)/layout.rs")
        .page("(shop)/cart/page.rs")
        .layout("(admin)/layout.rs")
        .page("(admin)/users/page.rs");

    let scanned = walk_tree(&tree);
    let shop = root_layout(scanned.entry("(shop)/cart/page.rs"), scanned.entries());
    let admin = root_layout(scanned.entry("(admin)/users/page.rs"), scanned.entries());

    assert_eq!(
        shop.map(|l| l.file.clone()),
        Some(format!("{}/(shop)/layout.rs", scanned.root))
    );
    assert_eq!(
        admin.map(|l| l.file.clone()),
        Some(format!("{}/(admin)/layout.rs", scanned.root))
    );
}

/// `@{children}` in a root layout serves the same markup as `<slot />` but
/// anchors no `<template pp-component>`, so the document ships without its
/// reactive boundary and nothing looks wrong. Only the build knows the
/// layout is a root, so only the build can refuse it.
#[test]
#[should_panic(expected = "root layout with no `<slot />`")]
fn a_root_layout_interpolating_children_is_refused() {
    let tree = Tree::new("root-noslot");
    tree.page("page.rs").file(
        "layout.rs",
        "pub fn layout(children: Html) -> Html {\n\
         \x20   html! {r##\"<html><body>@{children}</body></html>\"##}\n\
         }\n",
    );
    scan(tree.path());
}

#[test]
fn a_root_layout_with_a_slot_is_kept() {
    let tree = Tree::new("root-slot");
    tree.page("page.rs").file(
        "layout.rs",
        "pub fn layout(children: Html) -> Html {\n\
         \x20   html! {r##\"<html><body><slot /></body></html>\"##}\n\
         }\n",
    );

    assert!(walk_tree(&tree).entry("layout.rs").root);
}

/// A `<slot />` named in a comment is prose, not markup — it satisfies
/// nothing.
#[test]
#[should_panic(expected = "root layout with no `<slot />`")]
fn a_slot_in_a_comment_does_not_count() {
    let tree = Tree::new("root-comment-slot");
    tree.page("page.rs").file(
        "layout.rs",
        "// The page renders at `<slot />` below.\n\
         pub fn layout(children: Html) -> Html {\n\
         \x20   html! {r##\"<html><body>@{children}</body></html>\"##}\n\
         }\n",
    );
    scan(tree.path());
}

/// A page with no document of its own, in an app that has two: navigating to
/// it would swap its markup into whichever shell happened to be on screen.
#[test]
#[should_panic(expected = "every page needs one")]
fn a_page_outside_every_root_layout_is_refused() {
    let tree = Tree::new("root-orphan");
    tree.layout("(shop)/layout.rs")
        .page("(shop)/cart/page.rs")
        .layout("(admin)/layout.rs")
        .page("(admin)/users/page.rs")
        .page("stray/page.rs");

    walk_tree(&tree);
}

/// One root layout is the ordinary case, and a page outside it is a fragment
/// the author chose. Nothing to compare, so nothing to refuse.
#[test]
fn a_page_outside_the_only_root_layout_is_allowed() {
    let tree = Tree::new("root-single");
    tree.layout("(shop)/layout.rs")
        .page("(shop)/cart/page.rs")
        .page("stray/page.rs");

    let scanned = walk_tree(&tree);
    assert!(root_layout(scanned.entry("stray/page.rs"), scanned.entries()).is_none());
}
