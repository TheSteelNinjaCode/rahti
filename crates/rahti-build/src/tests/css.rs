//! The stylesheet build: which asset a machine asks for, and when the
//! compile runs at all.

use super::*;
use crate::tree::Tree;

fn css() -> Css {
    Css {
        entry: "src/app/globals.css".into(),
        output: "public/css/styles.css".into(),
        version: "4.3.3".into(),
        download: false,
        minify: false,
        sha256: None,
        engine: Engine::Tailwind,
    }
}

// ------------------------------------------------------------------- assets

#[test]
fn every_supported_platform_names_an_asset() {
    assert_eq!(
        asset_name("windows", "x86_64").unwrap(),
        "tailwindcss-windows-x64.exe"
    );
    assert_eq!(
        asset_name("windows", "aarch64").unwrap(),
        "tailwindcss-windows-arm64.exe"
    );
    assert_eq!(
        asset_name("linux", "x86_64").unwrap(),
        "tailwindcss-linux-x64"
    );
    assert_eq!(
        asset_name("linux", "aarch64").unwrap(),
        "tailwindcss-linux-arm64"
    );
    assert_eq!(
        asset_name("macos", "x86_64").unwrap(),
        "tailwindcss-macos-x64"
    );
    assert_eq!(
        asset_name("macos", "aarch64").unwrap(),
        "tailwindcss-macos-arm64"
    );
}

/// Tailwind publishes six binaries. Anything else has to say so rather than
/// guess a URL that will 404 halfway through a build.
#[test]
fn an_unpublished_platform_names_nothing() {
    assert!(asset_name("freebsd", "x86_64").is_none());
    assert!(asset_name("linux", "riscv64").is_none());
    assert!(platform_key("freebsd", "x86_64").is_none());
}

/// The key a checksum is recorded under is the asset name without the
/// `tailwindcss-` prefix and the extension — so a hash pasted from the build
/// warning lands under the platform it was measured on.
#[test]
fn the_checksum_key_matches_the_asset() {
    for (os, arch) in [
        ("windows", "x86_64"),
        ("linux", "aarch64"),
        ("macos", "aarch64"),
    ] {
        let key = platform_key(os, arch).unwrap();
        let asset = asset_name(os, arch).unwrap();
        assert!(
            asset.starts_with(&format!("tailwindcss-{key}")),
            "{asset} does not carry the key {key}"
        );
    }
}

/// Only Windows gets a suffix, and the cache name carries the version so a
/// bumped `version` is not served the previous download.
#[test]
fn the_cache_name_is_version_stamped() {
    let name = binary_name("4.3.3");
    assert!(name.starts_with("tailwindcss-4.3.3"), "got {name}");
    assert_ne!(binary_name("4.3.3"), binary_name("4.4.0"));
}

// ------------------------------------------------------------------ minify

/// The setting alone decides. If the profile got a say, a release build would
/// commit a minified sheet and the next debug build would commit it back.
#[test]
fn minifying_is_a_setting_not_a_profile() {
    let mut css = css();

    css.minify = true;
    assert!(minify(&css));

    css.minify = false;
    assert!(!minify(&css), "nothing outside the setting may turn it on");
}

// ---------------------------------------------------------------- publishing

/// The regression: a build read a half-written scratch file, got nothing, and
/// published nothing over a working stylesheet. Pages rendered unstyled with
/// no error to explain it.
#[test]
fn an_empty_result_never_replaces_a_working_sheet() {
    assert!(would_erase("", ".a{color:red}"));
    assert!(
        would_erase("   \n\t ", ".a{color:red}"),
        "whitespace is empty"
    );
}

/// The guard is about not destroying work, so it must not fire when there is
/// no work to destroy — a first build, or an entry that really is empty.
#[test]
fn an_empty_result_over_nothing_is_allowed() {
    assert!(!would_erase("", ""));
    assert!(!would_erase("", "  \n "));
    assert!(!would_erase(".a{color:red}", ".b{color:blue}"));
}

/// Two builds must not share a scratch file: `cargo watch` overlaps them, and
/// Tailwind truncates its output before filling it.
#[test]
fn the_scratch_file_is_unique_to_this_build() {
    let path = tmp_output();
    assert!(path.starts_with(TMP_DIR), "{path:?} escaped {TMP_DIR}");
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.contains(&std::process::id().to_string()),
        "{name} is not stamped with the process id"
    );
}

// ------------------------------------------------------------------ running

/// Two ways to want no stylesheet build, and neither is an error: an app that
/// never opted into Tailwind, and one that opted back out.
#[test]
fn a_missing_entry_compiles_nothing() {
    let mut css = css();
    css.entry = "src/app/does-not-exist.css".into();
    // No CLI is resolved and no output is written, so reaching the end is
    // the whole assertion.
    build(&css);
}

#[test]
fn the_none_engine_builds_nothing() {
    let mut css = css();
    css.engine = Engine::None;
    css.entry = file!().into(); // exists, and would otherwise be compiled
    build(&css);
}

// ------------------------------------------------------------- plain engine

/// The engine's whole promise: an app that brings its own CSS gets its file
/// published without a compiler being fetched to copy it.
#[test]
fn the_plain_engine_publishes_the_entry_as_it_is() {
    let tree = Tree::new("plain");
    let entry = tree.path().join("globals.css");
    let output = tree.path().join("styles.css");
    std::fs::write(&entry, ".card { color: red }\n").unwrap();

    let mut css = css();
    css.engine = Engine::Plain;
    css.entry = entry.to_string_lossy().into();
    css.output = output.to_string_lossy().into();

    build(&css);
    assert_eq!(tree.read("styles.css"), ".card { color: red }\n");
}

// ------------------------------------------------- tailwind in a plain sheet

/// Copied verbatim, these are not stylesheet rules — `@import "tailwindcss"`
/// sends the browser after a file that is not there, and the rest silently do
/// nothing.
#[test]
fn a_tailwind_directive_is_found() {
    assert_eq!(
        tailwind_directive("@import \"tailwindcss\" source(none);\n"),
        Some("@import \"tailwindcss\"")
    );
    assert_eq!(tailwind_directive("  @source \"../\";\n"), Some("@source"));
    assert_eq!(
        tailwind_directive(".a{}\n@theme inline {\n"),
        Some("@theme")
    );
}

#[test]
fn ordinary_css_carries_no_directive() {
    assert_eq!(
        tailwind_directive(":root { --bg: #fff }\n.a { color: red }\n"),
        None
    );
    assert_eq!(
        tailwind_directive("@media (min-width: 40rem) { .a {} }\n"),
        None
    );
    assert_eq!(tailwind_directive("@import \"./other.css\";\n"), None);
}

/// The word belongs in a comment explaining a migration, and a comment is not
/// a directive.
#[test]
fn the_word_in_a_comment_is_not_a_directive() {
    assert_eq!(
        tailwind_directive("/* was @import \"tailwindcss\" before the switch */\n"),
        None
    );
}

#[test]
#[should_panic(expected = "still contains")]
fn the_plain_engine_refuses_a_tailwind_entry() {
    let tree = Tree::new("plain-tailwind");
    let entry = tree.path().join("globals.css");
    std::fs::write(&entry, "@import \"tailwindcss\";\n.a { color: red }\n").unwrap();

    let mut css = css();
    css.engine = Engine::Plain;
    css.entry = entry.to_string_lossy().into();
    css.output = tree.path().join("styles.css").to_string_lossy().into();

    build(&css);
}
