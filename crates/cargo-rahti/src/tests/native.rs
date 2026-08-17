//! Delegation to the optional packaging tool.

use super::*;

#[test]
fn the_missing_message_names_the_command_that_fixes_it() {
    let message = missing();
    assert!(
        message.contains("cargo install cargo-rahti-native"),
        "{message}"
    );
    // And says why it is not already there, so "optional" is a fact rather
    // than an apology.
    assert!(message.contains("optional"), "{message}");
}

#[test]
fn the_missing_message_offers_no_way_to_install_it_for_you() {
    // A tool that reacted to a missing dependency by fetching and running one
    // is a tool that decides what runs on somebody's machine.
    let message = missing().to_ascii_lowercase();
    for forbidden in ["installing", "downloading", "shall i", "would you like"] {
        assert!(!message.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn the_helper_is_looked_for_beside_this_executable_first() {
    // `cargo install` puts both binaries in one directory, and a workspace
    // checkout puts both in one `target/debug` — so a local build of one
    // should reach the local build of the other rather than an installed
    // version.
    let path = locate();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert_eq!(name, format!("{HELPER}{}", std::env::consts::EXE_SUFFIX));

    // In this workspace the sibling exists whenever it has been built, and
    // then the path is absolute rather than a bare name for `PATH` to resolve.
    let sibling = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("{HELPER}{}", std::env::consts::EXE_SUFFIX));
    if sibling.is_file() {
        assert_eq!(path, sibling);
    }
}

/// The real thing, end to end, when the helper has been built.
///
/// Both are workspace members, so a `cargo test --workspace` run has the
/// binary — but `cargo test -p cargo-rahti` alone may not, and a test that
/// failed because of what else had been built would be a bad test. So it
/// checks and says which it did.
#[test]
fn a_forwarded_command_reaches_the_helper() {
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("{HELPER}{}", std::env::consts::EXE_SUFFIX));

    if !helper.is_file() {
        eprintln!(
            "skipped: {} has not been built — run `cargo build --workspace` first",
            helper.display()
        );
        return;
    }

    // What `cargo rahti native --help` forwards.
    let output = std::process::Command::new(&helper)
        .args(["native", "--help"])
        .output()
        .expect("the helper runs");

    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("cargo rahti native init"), "{text}");
    assert!(text.contains("--target"), "{text}");
}

#[test]
fn a_forwarded_failure_is_forwarded_as_a_failure() {
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("{HELPER}{}", std::env::consts::EXE_SUFFIX));

    if !helper.is_file() {
        eprintln!("skipped: the helper has not been built");
        return;
    }

    let output = std::process::Command::new(&helper)
        .args(["native", "build", "--target", "ios"])
        .output()
        .expect("the helper runs");

    assert!(!output.status.success(), "an unsupported target succeeded");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("ios"), "{text}");
}
