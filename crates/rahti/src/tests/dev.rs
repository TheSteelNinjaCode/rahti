//! Live reload: the flag, the client tag, and what counts as a stylesheet.

use super::*;

/// The tag `insert_client` writes, for an assertion to look for.
fn tag() -> String {
    format!("<script {MARKER} src=\"{SCRIPT_PATH}\"></script>")
}

// -------------------------------------------------------------------- flag

#[test]
fn the_flag_reads_the_usual_yes_words() {
    for value in ["1", "true", "on", "yes", " true ", "yes\n"] {
        assert!(flag(value), "{value:?}");
    }
}

/// Anything that is not an "on" word is off, so `RAHTI_DEV=0` in a debug
/// build turns live reload off.
#[test]
fn anything_else_turns_it_off() {
    for value in ["0", "false", "off", "no", "", "TRUE", "maybe"] {
        assert!(!flag(value), "{value:?}");
    }
}

// ------------------------------------------------------------------ client

#[test]
fn the_client_goes_just_before_the_body_closes() {
    let out = insert_client("<html><body><p>x</p></body></html>".to_string());
    assert_eq!(out, format!("<html><body><p>x</p>{}</body></html>", tag()));
}

/// A `</body>` written inside page content must not capture the tag, so the
/// last one wins.
#[test]
fn a_body_tag_in_the_content_does_not_capture_it() {
    let out = insert_client("<html><body><code>&lt;/body&gt;</code></body></html>".to_string());
    let at = out.find(&tag()).expect("the client is inserted");
    assert!(
        out[at..].starts_with(&format!("{}</body></html>", tag())),
        "{out}"
    );
}

/// A document that omits the body close tag still gets one.
#[test]
fn a_document_without_a_body_close_falls_back_to_html() {
    let out = insert_client("<html><p>x</p></html>".to_string());
    assert_eq!(out, format!("<html><p>x</p>{}</html>", tag()));
}

/// A fragment is not a document, and appending is the only thing left.
#[test]
fn a_fragment_gets_the_client_appended() {
    let out = insert_client("<p>x</p>".to_string());
    assert_eq!(out, format!("<p>x</p>{}", tag()));
}

/// `apply_layout` runs once per layout in the chain, so a page with nested
/// layouts passes through injection more than once.
#[test]
fn a_document_that_already_has_the_client_is_left_alone() {
    let once = insert_client("<html><body>x</body></html>".to_string());
    assert_eq!(inject(once.clone()), once, "a second pass adds nothing");
    assert_eq!(once.matches("__rahti/dev.js").count(), 1);
}

// ---------------------------------------------------------------- watching

/// A stylesheet can be swapped in place; anything else needs the page back.
#[test]
fn only_stylesheets_can_be_hot_swapped() {
    assert!(is_css(&PathBuf::from("public/css/styles.css")));
    assert!(
        is_css(&PathBuf::from("public/CSS/STYLES.CSS")),
        "extensions are case-insensitive"
    );
    assert!(!is_css(&PathBuf::from("public/js/main.js")));
    assert!(!is_css(&PathBuf::from("public/index.html")));
    assert!(
        !is_css(&PathBuf::from("public/css")),
        "a directory is not a stylesheet"
    );
}

#[test]
fn the_two_signals_are_named_for_the_client() {
    assert_eq!(Signal::Css.name(), "css");
    assert_eq!(Signal::Reload.name(), "reload");
}

/// Any new value means "you are looking at output from a server that no
/// longer exists", so it has to be stable within a process and unique across
/// one restart.
#[test]
fn the_boot_id_is_fixed_for_the_life_of_the_process() {
    assert_eq!(boot_id(), boot_id());
    assert!(boot_id().contains('-'), "{}", boot_id());
    assert!(!boot_id().is_empty());
}

/// Outside dev mode the router still wires this in, so it has to be safe to
/// build either way.
#[test]
fn the_dev_routes_build() {
    let _ = routes(PathBuf::from("public"));
}

// ------------------------------------------------------------ diagnostics

#[test]
fn browser_diagnostics_are_bounded_and_given_server_context() {
    let oversized = "x".repeat(MAX_LOG_FIELD + 10);
    let body = serde_json::to_vec(&json!([{
        "level": "error",
        "message": oversized,
        "url": "http://127.0.0.1:3000/counter",
        "source": "http://127.0.0.1:3000/counter",
        "line": 12,
        "column": 7,
        "ignored": "not copied"
    }]))
    .unwrap();

    let entries = browser_entries(&body).expect("valid diagnostics");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["kind"], "browser");
    assert_eq!(entry["level"], "error");
    assert_eq!(entry["line"], 12);
    assert!(entry["message"].as_str().unwrap().ends_with("…[truncated]"));
    assert!(entry.get("ignored").is_none());
    assert!(entry.get("receivedAtMs").is_some());
    assert_eq!(entry["boot"], boot_id());
}

#[test]
fn malformed_browser_diagnostics_are_refused() {
    assert!(browser_entries(b"not-json").is_err());
    assert!(browser_entries(br#"{"message":"not an array"}"#).is_err());
}

#[test]
fn one_request_cannot_write_an_unbounded_number_of_entries() {
    let values: Vec<Value> = (0..MAX_LOG_ENTRIES + 10)
        .map(|n| json!({ "level": "warn", "message": n.to_string() }))
        .collect();
    let body = serde_json::to_vec(&values).unwrap();
    assert_eq!(browser_entries(&body).unwrap().len(), MAX_LOG_ENTRIES);
}

#[test]
fn a_source_location_reads_like_a_browser_location() {
    let entry = json!({
        "source": "http://127.0.0.1:3000/counter",
        "line": 14,
        "column": 9
    });
    assert_eq!(source_of(&entry), " — http://127.0.0.1:3000/counter:14:9");
}

const MANIFEST: &str = r#"{
  "routes": [{ "url": "/", "component": "page_33bf1147", "file": "src/app/page.rs" }],
  "layouts": [{ "component": "layout_aa", "file": "src/app/layout.rs" }],
  "errors": [],
  "loading": [{ "component": "loading_bb", "file": "src/app/loading.rs" }]
}"#;

fn entry(fields: &[(&str, &str)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect()
}

/// A diagnostic that names a component is tied to the file it came from.
#[test]
fn a_named_component_resolves_to_its_file() {
    let files = files_from_manifest(MANIFEST);
    assert_eq!(files.len(), 3);
    let from_field = entry(&[("component", "page_33bf1147 (main)")]);
    assert_eq!(
        source_file(&from_field, &files).as_deref(),
        Some("src/app/page.rs")
    );
}

/// So does one whose stack runs through a compiled component script.
#[test]
fn a_component_stack_frame_resolves_to_its_file() {
    let files = files_from_manifest(MANIFEST);
    let from_stack = entry(&[(
        "stack",
        "TypeError: x is undefined\n    at walkIn (pp://component/loading_bb.js:12:7)",
    )]);
    assert_eq!(
        source_file(&from_stack, &files).as_deref(),
        Some("src/app/loading.rs")
    );

    let unknown = entry(&[("stack", "at pp://component/nobody.js:1:1")]);
    assert_eq!(source_file(&unknown, &files), None);
    assert!(files_from_manifest("not json").is_empty());
}
