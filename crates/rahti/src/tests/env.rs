//! The `.env` parser.
//!
//! `parse` rather than `load_env`, deliberately: the setting half writes to
//! the process environment, which every test in the binary shares, so it is
//! not a thing to exercise from a test.

use super::*;

fn parsed(contents: &str) -> Vec<(String, String)> {
    parse(contents)
}

#[test]
fn reads_an_ordinary_pair() {
    assert_eq!(
        parsed("DATABASE_URL=sqlite://./app.db"),
        [("DATABASE_URL".into(), "sqlite://./app.db".into())]
    );
}

#[test]
fn strips_the_quotes_a_secret_is_usually_written_in() {
    // The generated `AUTH_SECRET` is base64, so it carries `+`, `/` and `=`,
    // and the scaffold quotes it. The quotes are syntax, not value.
    let secret = "j0it2PS/rKI8Idz/JQgkH+TVqaJTHjv28gOpBhe2XnD5=";
    assert_eq!(
        parsed(&format!("AUTH_SECRET=\"{secret}\"")),
        [("AUTH_SECRET".into(), secret.to_string())]
    );
    assert_eq!(
        parsed(&format!("AUTH_SECRET='{secret}'")),
        [("AUTH_SECRET".into(), secret.to_string())]
    );
}

/// A value with `=` in it is one value: only the first `=` separates.
#[test]
fn a_value_may_contain_the_separator() {
    assert_eq!(
        parsed("AUTH_SECRET=abc=def=="),
        [("AUTH_SECRET".into(), "abc=def==".into())]
    );
}

#[test]
fn ignores_comments_and_blank_lines() {
    let contents = "\n# a comment\n\n  # an indented comment\nA=1\n";
    assert_eq!(parsed(contents), [("A".into(), "1".into())]);
}

#[test]
fn accepts_an_export_prefix() {
    assert_eq!(parsed("export A=1"), [("A".into(), "1".into())]);
}

#[test]
fn skips_a_line_that_is_not_a_pair() {
    assert_eq!(
        parsed("nonsense\n=novalue\nA=1"),
        [("A".into(), "1".into())]
    );
}

#[test]
fn keeps_the_order_the_file_is_written_in() {
    let contents = "AUTH_SECRET=s\nAUTH_COOKIE_NAME=c\nSESSION_LIFETIME_HOURS=7\n";
    assert_eq!(
        parsed(contents),
        [
            ("AUTH_SECRET".into(), "s".into()),
            ("AUTH_COOKIE_NAME".into(), "c".into()),
            ("SESSION_LIFETIME_HOURS".into(), "7".into()),
        ]
    );
}
