//! Names, and the hash they are built from.
//!
//! These identifiers ship inside checked-in markup and in client code, so the
//! vectors below are golden: the same path has to hash the same on every
//! machine and every release. `rahti-build`'s `route.rs` reimplements this and
//! carries the same vectors — if one side moves, the two test suites disagree
//! and the client runtime stops finding its blocks.

use super::*;

// ------------------------------------------------------------------ hashing

#[test]
fn fnv1a32_matches_the_published_vectors() {
    // The reference vectors for FNV-1a, 32-bit.
    assert_eq!(fnv1a32(""), 0x811c_9dc5);
    assert_eq!(fnv1a32("a"), 0xe40c_292c);
    assert_eq!(fnv1a32("foobar"), 0xbf9c_f968);
}

#[test]
fn a_path_always_hashes_the_same() {
    assert_eq!(
        identifier("src/app/page.rs").unwrap(),
        format!("page_{:08x}", fnv1a32("page.rs"))
    );
    assert_eq!(
        identifier("src/app/users/page.rs").unwrap(),
        format!("page_{:08x}", fnv1a32("users/page.rs"))
    );
}

// -------------------------------------------------------- convention files

#[test]
fn every_convention_file_has_a_prefix() {
    assert!(identifier("src/app/page.rs").unwrap().starts_with("page_"));
    assert!(
        identifier("src/app/layout.rs")
            .unwrap()
            .starts_with("layout_")
    );
    assert!(
        identifier("src/app/error.rs")
            .unwrap()
            .starts_with("error_")
    );
    // `not-found` is not a legal identifier, so it is underscored.
    assert!(
        identifier("src/app/not-found.rs")
            .unwrap()
            .starts_with("not_found_")
    );
}

#[test]
fn every_name_is_a_javascript_identifier() {
    for file in [
        "src/app/page.rs",
        "src/app/layout.rs",
        "src/app/not-found.rs",
        "src/app/customers/[id]/page.rs",
        "src/app/docs/[[...slug]]/page.rs",
        "src/app/posts/[...id]/page.rs",
        "src/app/(marketing)/layout.rs",
    ] {
        let id = identifier(file).unwrap_or_else(|| panic!("{file} is a convention file"));
        assert!(is_valid_js_identifier(&id), "{file} -> {id}");
    }
}

/// Directory names carrying brackets and parentheses is exactly why the name
/// comes from a hash rather than from the path segments.
#[test]
fn bracketed_segments_produce_distinct_names() {
    let dynamic = identifier("src/app/customers/[id]/page.rs").unwrap();
    let literal = identifier("src/app/customers/id/page.rs").unwrap();
    assert_ne!(dynamic, literal);
}

#[test]
fn a_file_that_is_not_a_convention_has_no_name() {
    assert_eq!(identifier("src/app/helpers.rs"), None);
    assert_eq!(identifier("src/components/user_card.rs"), None);
    assert_eq!(identifier("src/main.rs"), None);
}

/// Spans report whatever path rustc was given, which is absolute on some
/// invocations and uses backslashes on Windows.
#[test]
fn the_name_does_not_depend_on_how_the_path_was_written() {
    let relative = identifier("src/app/users/page.rs").unwrap();
    assert_eq!(
        identifier("C:/work/proj/src/app/users/page.rs").unwrap(),
        relative
    );
    assert_eq!(
        identifier(r"C:\work\proj\src\app\users\page.rs").unwrap(),
        relative
    );
    assert_eq!(
        identifier("/home/ada/proj/src/app/users/page.rs").unwrap(),
        relative
    );
}

// ------------------------------------------------------------- components

/// A component is named after its function, since one file may hold several.
#[test]
fn two_components_in_one_file_get_different_names() {
    let card = function_identifier("src/components/user_card.rs", "UserCard");
    let badge = function_identifier("src/components/user_card.rs", "Badge");
    assert_ne!(card, badge);
    assert!(card.starts_with("user_card_"), "{card}");
    assert!(badge.starts_with("badge_"), "{badge}");
}

/// The hash covers the path too, so the same function name in two files does
/// not collide.
#[test]
fn the_same_name_in_two_files_does_not_collide() {
    assert_ne!(
        function_identifier("src/components/a.rs", "Card"),
        function_identifier("src/components/b.rs", "Card"),
    );
}

#[test]
fn a_component_name_does_not_depend_on_the_absolute_path() {
    let here = function_identifier("src/components/user_card.rs", "UserCard");
    assert_eq!(
        function_identifier("C:/work/proj/src/components/user_card.rs", "UserCard"),
        here
    );
    assert_eq!(
        function_identifier(r"D:\other\src\components\user_card.rs", "UserCard"),
        here
    );
}

#[test]
fn a_component_name_is_always_a_javascript_identifier() {
    for name in ["UserCard", "Button", "HTMLBadge", "X", "Ünicode", "Class"] {
        let id = function_identifier("src/components/x.rs", name);
        assert!(is_valid_js_identifier(&id), "{name} -> {id}");
    }
}

#[test]
fn snake_case_breaks_words_the_way_a_reader_would() {
    assert_eq!(snake_case("UserCard"), "user_card");
    assert_eq!(snake_case("Button"), "button");
    // The last capital of a run starts the next word.
    assert_eq!(snake_case("HTMLBadge"), "html_badge");
    assert_eq!(snake_case("A"), "a");
    assert_eq!(snake_case("Card2Row"), "card2_row");
    assert_eq!(snake_case("Already_Split"), "already_split");
}

// ------------------------------------------------------------ identifiers

#[test]
fn identifiers_accept_what_javascript_accepts() {
    for name in ["a", "_x", "$", "$a1", "userCard", "page_9f2b41c7", "Class"] {
        assert!(is_valid_js_identifier(name), "{name}");
    }
}

#[test]
fn identifiers_reject_what_javascript_rejects() {
    for name in ["", "1a", "my-block", "a b", "a.b", "über", "[id]"] {
        assert!(!is_valid_js_identifier(name), "{name}");
    }
}

/// A reserved word is a syntax error where these are used, not a variable.
#[test]
fn reserved_words_are_rejected() {
    for name in [
        "class", "return", "new", "typeof", "await", "this", "true", "null",
    ] {
        assert!(!is_valid_js_identifier(name), "{name}");
    }
}
