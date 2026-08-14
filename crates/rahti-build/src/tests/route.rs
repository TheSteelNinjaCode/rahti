//! How a path on disk becomes a URL, and a name.

use super::*;

/// `to_urls`, but readable in an assertion.
fn urls(dirs: &[&str]) -> Vec<String> {
    to_urls(&dirs.iter().map(|d| (*d).to_string()).collect::<Vec<_>>())
}

fn chain(dirs: &[&str]) -> Vec<String> {
    dirs.iter().map(|d| (*d).to_string()).collect()
}

// -------------------------------------------------------------------- kind

#[test]
fn convention_file_names_are_recognised() {
    assert_eq!(Kind::from_file_name("page.rs"), Some(Kind::Page));
    assert_eq!(Kind::from_file_name("route.rs"), Some(Kind::Route));
    assert_eq!(Kind::from_file_name("layout.rs"), Some(Kind::Layout));
    assert_eq!(Kind::from_file_name("error.rs"), Some(Kind::Error));
    assert_eq!(Kind::from_file_name("not-found.rs"), Some(Kind::NotFound));
}

#[test]
fn anything_else_is_ordinary_code() {
    assert_eq!(Kind::from_file_name("helpers.rs"), None);
    assert_eq!(Kind::from_file_name("Page.rs"), None, "the match is exact");
    assert_eq!(
        Kind::from_file_name("not_found.rs"),
        None,
        "the file is hyphenated"
    );
    assert_eq!(Kind::from_file_name("mod.rs"), None);
}

/// Only a page and a route answer a URL. A layout wraps one, an error
/// boundary catches one, and neither is reachable on its own.
#[test]
fn only_pages_and_routes_produce_urls() {
    assert!(Kind::Page.is_routable());
    assert!(Kind::Route.is_routable());
    assert!(!Kind::Layout.is_routable());
    assert!(!Kind::Error.is_routable());
    assert!(!Kind::NotFound.is_routable());
}

// ---------------------------------------------------------------- segments

#[test]
fn a_plain_directory_is_a_literal_segment() {
    assert!(matches!(parse_segment("users"), Segment::Literal(name) if name == "users"));
}

#[test]
fn brackets_mark_a_parameter() {
    assert!(matches!(parse_segment("[id]"), Segment::Dynamic(name) if name == "id"));
    assert!(matches!(parse_segment("[...id]"), Segment::CatchAll(name) if name == "id"));
    assert!(matches!(parse_segment("[[...slug]]"), Segment::OptionalCatchAll(n) if n == "slug"));
}

#[test]
fn parentheses_mark_a_group() {
    assert!(matches!(parse_segment("(marketing)"), Segment::Group));
}

/// `[[...x]]` also matches the test for `[x]`, so the order the cases are
/// tried in is what keeps them apart.
#[test]
fn the_longest_bracket_form_wins() {
    assert!(matches!(
        parse_segment("[[...slug]]"),
        Segment::OptionalCatchAll(_)
    ));
    assert!(matches!(parse_segment("[...slug]"), Segment::CatchAll(_)));
}

// -------------------------------------------------------------------- urls

#[test]
fn the_app_root_is_the_index() {
    assert_eq!(urls(&[]), vec!["/"]);
}

#[test]
fn directories_become_path_segments() {
    assert_eq!(urls(&["users"]), vec!["/users"]);
    assert_eq!(urls(&["api", "health"]), vec!["/api/health"]);
}

#[test]
fn a_dynamic_segment_becomes_a_parameter() {
    assert_eq!(urls(&["customers", "[id]"]), vec!["/customers/{id}"]);
}

#[test]
fn a_catch_all_takes_one_or_more_segments() {
    assert_eq!(urls(&["posts", "[...id]"]), vec!["/posts/{*id}"]);
}

/// An optional catch-all answers its parent path as well as everything below
/// it, which is two routes rather than one.
#[test]
fn an_optional_catch_all_answers_two_urls() {
    assert_eq!(
        urls(&["docs", "[[...slug]]"]),
        vec!["/docs", "/docs/{*slug}"]
    );
}

/// A group organises files without appearing in the URL.
#[test]
fn a_group_contributes_no_segment() {
    assert_eq!(urls(&["(marketing)", "adds"]), vec!["/adds"]);
    assert_eq!(urls(&["(marketing)"]), vec!["/"]);
}

#[test]
fn segments_combine() {
    assert_eq!(
        urls(&["(shop)", "customers", "[id]", "orders", "[...rest]"]),
        vec!["/customers/{id}/orders/{*rest}"]
    );
}

// ------------------------------------------------------------------ covers

/// Compared as raw directory names, so a layout inside a group still covers
/// its subtree even though the group contributes no URL.
#[test]
fn an_ancestor_covers_its_subtree() {
    assert!(
        covers(&[], &chain(&["users"])),
        "the root layout covers everything"
    );
    assert!(
        covers(&chain(&["users"]), &chain(&["users"])),
        "and its own segment"
    );
    assert!(covers(
        &chain(&["(marketing)"]),
        &chain(&["(marketing)", "adds"])
    ));
}

#[test]
fn a_sibling_does_not_cover() {
    assert!(!covers(&chain(&["users"]), &chain(&["customers"])));
    assert!(
        !covers(&chain(&["users", "a"]), &chain(&["users"])),
        "deeper than the page"
    );
    assert!(
        !covers(&chain(&["user"]), &chain(&["users"])),
        "not a prefix match on names"
    );
}

// -------------------------------------------------------------------- rank

#[test]
fn static_urls_rank_before_dynamic_ones() {
    assert!(rank("/users") < rank("/users/{id}"));
    assert!(rank("/users/{id}") < rank("/posts/{*id}"));
}

// ------------------------------------------------------------ module names

#[test]
fn a_module_name_says_what_it_points_at() {
    let mut used = Vec::new();
    assert_eq!(
        module_name(Kind::Page, &chain(&["users"]), &mut used),
        "p_users"
    );
    assert_eq!(
        module_name(Kind::Layout, &chain(&["users"]), &mut used),
        "l_users"
    );
    assert_eq!(
        module_name(Kind::Route, &chain(&["api", "health"]), &mut used),
        "r_api_health"
    );
    assert_eq!(module_name(Kind::Error, &chain(&[]), &mut used), "e_root");
    assert_eq!(
        module_name(Kind::NotFound, &chain(&[]), &mut used),
        "nf_root"
    );
}

/// Punctuation is stripped, so distinct paths can sanitize to the same name
/// — the second one is numbered rather than colliding.
#[test]
fn colliding_module_names_are_numbered() {
    let mut used = Vec::new();
    assert_eq!(
        module_name(Kind::Page, &chain(&["[id]"]), &mut used),
        "p_id"
    );
    assert_eq!(
        module_name(Kind::Page, &chain(&["id"]), &mut used),
        "p_id_2"
    );
    assert_eq!(
        module_name(Kind::Page, &chain(&["[[...id]]"]), &mut used),
        "p_id_3"
    );
}

#[test]
fn a_module_name_is_a_rust_identifier() {
    let mut used = Vec::new();
    for dirs in [
        vec!["(marketing)", "adds"],
        vec!["docs", "[[...slug]]"],
        vec!["posts", "[...id]"],
    ] {
        let name = module_name(Kind::Page, &chain(&dirs), &mut used);
        assert!(
            name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_numeric()),
            "{dirs:?} -> {name}"
        );
    }
}

// ----------------------------------------------------------- component ids

/// The reference vectors for FNV-1a, 32-bit.
///
/// `rahti-macros` hashes these names too and asserts the same three values.
/// The two implementations have no way to call each other, so this pair of
/// tests is what holds them together: the macro derives a name from the
/// source path at expansion time, this side reports the same name in the
/// manifest, and a client runtime that finds neither is what a drift here
/// would cost.
#[test]
fn fnv1a32_matches_the_published_vectors() {
    assert_eq!(fnv1a32(""), 0x811c_9dc5);
    assert_eq!(fnv1a32("a"), 0xe40c_292c);
    assert_eq!(fnv1a32("foobar"), 0xbf9c_f968);
}

#[test]
fn a_component_id_is_its_kind_and_the_hash_of_its_path() {
    assert_eq!(
        component_id(Kind::Page, "page.rs"),
        format!("page_{:08x}", fnv1a32("page.rs"))
    );
    assert_eq!(
        component_id(Kind::Layout, "(marketing)/layout.rs"),
        format!("layout_{:08x}", fnv1a32("(marketing)/layout.rs"))
    );
}

#[test]
fn every_kind_has_a_prefix() {
    assert!(component_id(Kind::Page, "page.rs").starts_with("page_"));
    assert!(component_id(Kind::Layout, "layout.rs").starts_with("layout_"));
    assert!(component_id(Kind::Error, "error.rs").starts_with("error_"));
    assert!(component_id(Kind::Route, "route.rs").starts_with("route_"));
    // `not-found` is not a legal JavaScript identifier.
    assert!(component_id(Kind::NotFound, "not-found.rs").starts_with("not_found_"));
}

#[test]
fn two_paths_get_two_names() {
    assert_ne!(
        component_id(Kind::Page, "customers/[id]/page.rs"),
        component_id(Kind::Page, "customers/id/page.rs"),
    );
    assert_ne!(
        component_id(Kind::Page, "page.rs"),
        component_id(Kind::Layout, "page.rs")
    );
}

/// The value is used verbatim as a JavaScript variable name.
#[test]
fn a_component_id_is_always_an_identifier() {
    for relative in [
        "page.rs",
        "customers/[id]/page.rs",
        "docs/[[...slug]]/page.rs",
        "(marketing)/layout.rs",
        "not-found.rs",
    ] {
        let id = component_id(Kind::Page, relative);
        assert!(
            id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{relative} -> {id}"
        );
    }
}
