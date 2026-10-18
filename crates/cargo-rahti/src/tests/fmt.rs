use super::*;

#[test]
fn formatted_fixture_renders_the_same_html() {
    // These two macro bodies are also the formatter's input and exact expected
    // output below, so the real macro verifies the source transformation.
    let before = rahti::html! { <main><p>"Hello "{count}"!"</p><input disabled=@{false} aria-hidden=@{false} /><div class="a" @{..rahti::Attrs::new().set("class", "b")} class="c"></div></main> };
    let after = rahti::html! {
        <main>
            <p>"Hello "{count}"!"</p>
            <input disabled=@{false} aria-hidden=@{false} />
            <div class="a" @{..rahti::Attrs::new().set("class", "b")} class="c"></div>
        </main>
    };
    assert_eq!(before.as_str(), after.as_str());
    let source = include_str!("fmt.rs").replace("\r\n", "\n");
    let first = source.find("let before = rahti::html!").unwrap();
    let end = source[first..].find(";\n    let after").unwrap() + first;
    let original = &source[first..end];
    let expected_start = source[end..].find("let after = ").unwrap() + end;
    let expected_end = source[expected_start..].find(";\n    assert_eq!").unwrap() + expected_start;
    let expected = source[expected_start..expected_end].replace("let after", "let before");
    // Include the four-space containing-function indentation.
    assert_eq!(
        formatted(&format!("    {original}")),
        format!("    {expected}")
    );
}

fn formatted(source: &str) -> String {
    format_source(source).unwrap()
}

#[test]
fn nesting_and_mixed_children_are_readable_and_stable() {
    let source = "fn page() { html! { <main><div class=\"card\"><span>\"↟\"</span><div><small>\"ADVENTURE\"</small><strong>{stats.levelName}</strong></div></div><p>\"Score \"{score}\"!\"</p></main> }; }";
    let out = formatted(source);
    assert!(
        out.contains(
            "        <div class=\"card\">\n            <span>\"↟\"</span>\n            <div>"
        ),
        "{out}"
    );
    assert!(out.contains("<p>\"Score \"{score}\"!\"</p>"));
    assert_eq!(formatted(&out), out);
    assert!(out.starts_with("fn page() { html! {"));
    assert!(out.ends_with("}; }"));
}

#[test]
fn expressions_strings_raw_bodies_and_attribute_order_survive() {
    let source = r####"fn page() { html! { <main><button class="a" @{..attrs} class="b" onclick={save({nested: "<tag>", value: 2})} hidden={stats.shield > 0 || stats.mode !== "playing"}>@{value({ 1 })}</button><script>r###"
    const x = `line one
        line two`;
    const data = @{Json(&data)}; // comment
    const literal = '@@{x}';
"###</script><style>r##".x { color: "#fff"; }"##</style></main> }; }"####;
    let out = formatted(source);
    for preserved in [
        r#"onclick={save({nested: "<tag>", value: 2})}"#,
        r#"hidden={stats.shield > 0 || stats.mode !== "playing"}"#,
        "@{value({ 1 })}",
    ] {
        assert!(out.contains(preserved), "{out}");
    }
    for tag in ["script", "style"] {
        let body = source
            .split(&format!("<{tag}>"))
            .nth(1)
            .unwrap()
            .split(&format!("</{tag}>"))
            .next()
            .unwrap();
        assert!(out.contains(body));
    }
    assert!(out.find("class=\"a\"").unwrap() < out.find("@{..attrs}").unwrap());
    assert!(out.find("@{..attrs}").unwrap() < out.find("class=\"b\"").unwrap());
    assert_eq!(formatted(&out), out);
}

#[test]
fn comments_fragments_components_doctypes_and_namespaces_survive() {
    let source = r##"fn f() { html! { <><Card title=@{name}><slot /></Card>// sibling
<svg xml:space="preserve"><use xlink:href="#icon" /></svg>/* end */</> }; html! { <!DOCTYPE html><html><body>"Hi"</body></html> }; }"##;
    let out = formatted(source);
    assert!(out.contains("// sibling\n"));
    assert!(out.contains("/* end */"));
    assert!(out.contains("<Card title=@{name}>"));
    assert!(out.contains("<!DOCTYPE html>"));
    assert!(out.contains("xlink:href=\"#icon\""));
    assert_eq!(formatted(&out), out);
}

#[test]
fn bare_text_and_opening_tag_comments_are_left_intact() {
    let source = "fn f() { html! { <p>Hello <b>world</b> !</p> }; }";
    assert_eq!(formatted(source), source);
    let numeric = "fn f() { html! { <p>123 <b>world</b></p> }; }";
    assert_eq!(formatted(numeric), numeric);
    let mixed = "fn f() { html! { <p>\"Hello\" 123 <b>world</b></p> }; }";
    assert_eq!(formatted(mixed), mixed);
    let source = "fn f() { html! { <p /* keep */ class=\"x\">\"Hi\"</p> }; }";
    assert!(formatted(source).contains("<p /* keep */ class=\"x\">\"Hi\"</p>"));
}

#[test]
fn authored_workspace_sources_keep_tokens_and_are_idempotent() {
    fn fingerprint(stream: TokenStream, output: &mut Vec<String>) {
        for token in stream {
            match token {
                TokenTree::Group(g) => {
                    output.push(format!("open {:?}", g.delimiter()));
                    fingerprint(g.stream(), output);
                    output.push(format!("close {:?}", g.delimiter()));
                }
                // Tag punctuation can become separated when markup wraps,
                // while the punctuation characters themselves remain identical.
                TokenTree::Punct(p) => output.push(p.as_char().to_string()),
                other => output.push(other.to_string()),
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
    if !root.is_dir() {
        return;
    }
    let mut files = BTreeSet::new();
    collect(&root, &mut files).unwrap();
    // Published packages do not include the workspace demonstration sources.
    // The fixture tests above remain self-contained in the published crate.
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        let out = formatted(&source);
        assert_eq!(formatted(&out), out, "{}", path.display());
        let original_tokens: TokenStream = source.parse().unwrap();
        let formatted_tokens: TokenStream = out.parse().unwrap();
        let mut original = Vec::new();
        let mut actual = Vec::new();
        fingerprint(original_tokens, &mut original);
        fingerprint(formatted_tokens, &mut actual);
        assert_eq!(original, actual, "{}", path.display());
    }
}

#[test]
fn comments_and_strings_that_mention_macros_are_not_formatted() {
    let source = "// html! { <p></p> }\nconst X: &str = r#\"html! { <p></p> }\"#;";
    assert_eq!(formatted(source), source);
}

#[test]
fn crlf_unicode_and_existing_multiline_expressions_survive() {
    let source = "// 🌳\r\nfn f() { html! { <p title={one +\r\n two}>\"á\"</p> }; }\r\n";
    let out = formatted(source);
    assert!(out.contains("{one +\r\n two}"));
    assert!(!out.replace("\r\n", "").contains('\n'));
    assert_eq!(formatted(&out), out);
}

#[test]
fn long_tags_wrap_attributes_without_wrapping_bindings() {
    let out = formatted(
        r#"fn f() { html! { <button class="long-class-name-with-many-characters" onclick={save()} disabled={stats.shield > 0 || stats.mode !== "playing"}>"Go"</button> }; }"#,
    );
    assert!(out.contains("<button\n        class="), "{out}");
    assert!(out.contains("disabled={stats.shield > 0 || stats.mode !== \"playing\"}\n"));
    assert_eq!(formatted(&out), out);
}

#[test]
fn malformed_markup_and_rust_are_reported() {
    assert!(format_source("fn f() { html! { <div></span> }; }").is_err());
    assert!(format_source("fn f() { html! { <div> }; }").is_err());
    assert!(format_source("fn f() {").is_err());
}

#[test]
fn check_is_read_only_and_generated_files_are_excluded() {
    let root = std::env::temp_dir().join(format!("rahti-fmt-{}", std::process::id()));
    fs::create_dir_all(root.join("components")).unwrap();
    let source = "fn f() { html! { <main><p>\"Hi\"</p></main> }; }";
    let page = root.join("page.rs");
    fs::write(&page, source).unwrap();
    fs::write(root.join("routes.rs"), source).unwrap();
    fs::write(root.join("components/mod.rs"), source).unwrap();
    assert!(run(&["--check", root.to_str().unwrap()]).is_err());
    assert_eq!(fs::read_to_string(&page).unwrap(), source);
    run(&[root.to_str().unwrap()]).unwrap();
    run(&["--check", root.to_str().unwrap()]).unwrap();
    assert_eq!(fs::read_to_string(root.join("routes.rs")).unwrap(), source);
    assert_eq!(
        fs::read_to_string(root.join("components/mod.rs")).unwrap(),
        source
    );
    fs::write(&page, source).unwrap();
    let malformed = root.join("invalid.rs");
    fs::write(&malformed, "fn f() { html! { <p></div> }; }").unwrap();
    assert!(run(&[root.to_str().unwrap()]).is_err());
    assert_eq!(fs::read_to_string(&page).unwrap(), source);
    fs::remove_file(malformed).unwrap();
    // Remove only the files this test created, without recursive deletion.
    for path in [
        &page,
        &root.join("routes.rs"),
        &root.join("components/mod.rs"),
    ] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(root.join("components")).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn scaffold_teaches_the_formatting_commands_and_preservation_rules() {
    let guide = crate::templates::agents_md("example", false, false);
    for rule in [
        "cargo rahti fmt",
        "cargo rahti fmt --check",
        "script/style raw strings",
        "configuration-and-tooling.md",
    ] {
        assert!(guide.contains(rule));
    }
}
