//! One-character mistakes a coding agent makes in client code, and whether
//! `html!` stops them.
//!
//! Each case is a block that is wrong in exactly one place. The first test
//! holds the mistakes that are refused. The second is a ledger of the ones
//! that still compile: closing a gap makes its line fail, and the line then
//! moves to the first test.
//!
//! A payload's keys are refused by the compiler, through the constants
//! `html!` writes, so those cases assert on whether the keys were readable.

use crate::parse::{ClientKind, parse};
use crate::rpc_shapes::Shapes;
use proc_macro2::TokenStream;
use rahti_js::Code;
use std::str::FromStr;

const FILE: &str = "src/app/domains/page.rs";

fn shapes() -> Shapes {
    let text = "\
R\tsrc/app/domains/page.rs\tlist_domains\tDomain[]
R\tsrc/app/domains/page.rs\tadd_domain\tDomain
R\tsrc/app/domains/page.rs\trename_domain\tDomain
T\tsrc/app/domains/page.rs\tDomain\t{ id: number; name: string; owner: Owner | null }
T\tsrc/app/domains/page.rs\tOwner\t{ email: string }
";
    Shapes::parse(text, FILE)
}

/// A state that holds `list_domains`' response.
const LOAD: &str = r#"
    const [domains, setDomains] = pp.state([]);
    async function load() {
        setDomains(await pp.rpc("list_domains", {}));
    }
"#;

/// Rows of that state, reading one field.
fn rows(read: &str) -> String {
    format!(
        r#"<template pp-for="domain in domains"><li key={{domain.id}}>{{{read}}}</li></template>"#
    )
}

/// Whether `html!` itself refuses a block with this markup and script.
fn refused(markup: &str, script: &str) -> bool {
    let src = format!("<div>{markup}<script>r##\"{script}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let Ok(parsed) = parse(tokens) else {
        return true;
    };
    let shapes = shapes();
    let mut next = 0;
    for source in &parsed.client_code {
        let compiled = match source.kind {
            ClientKind::Expression => rahti_js::expression(&source.source).map(drop),
            ClientKind::Handler => rahti_js::handler(&source.source).map(drop),
            ClientKind::Script => rahti_js::component_script(&source.source).map(drop),
        };
        if compiled.is_err()
            || crate::rpc_calls::checks_against(source, &mut next, Some(&shapes)).is_err()
        {
            return true;
        }
    }
    crate::names::check(&parsed.client_code).is_err()
        || crate::state_types::check_against(&parsed.client_code, &shapes, None).is_err()
}

/// How much of the first `pp.rpc` payload in `script` can be checked:
/// `written` when every key is known, `unknown` when only the name is.
fn payload(script: &str) -> &'static str {
    let calls = rahti_js::rpc_calls(Code::Script, script);
    match &calls.first().expect("the script calls an rpc").payload {
        rahti_js::Payload::Missing => "none",
        rahti_js::Payload::Unchecked => "unknown",
        rahti_js::Payload::Object { open: true, .. } => "open",
        rahti_js::Payload::Object { .. } => "written",
    }
}

#[test]
fn mistakes_the_compiler_stops() {
    let cases: [(&str, String, String); 8] = [
        (
            "a misspelled field in a row of an rpc's response",
            rows("domain.nmae"),
            LOAD.into(),
        ),
        (
            "a misspelled nested field in a binding",
            "<p>{domains[0].owner.emial}</p>".into(),
            LOAD.into(),
        ),
        (
            "a misspelled field read straight off a result",
            String::new(),
            r#"async function add() {
                const made = await pp.rpc("add_domain", {});
                return made.nmae;
            }"#
            .into(),
        ),
        (
            "a misspelled field in the call's own .then",
            String::new(),
            r#"function add() {
                pp.rpc("add_domain", {}).then((made) => made.nmae);
            }"#
            .into(),
        ),
        (
            "a misspelled state name in a binding",
            r#"<p hidden={domians.length === 0}>"None"</p>"#.into(),
            LOAD.into(),
        ),
        (
            "a misspelled loop variable",
            rows("domian.name"),
            LOAD.into(),
        ),
        (
            "a misspelled function in a handler",
            r#"<button onclick={laod()}>"Reload"</button>"#.into(),
            LOAD.into(),
        ),
        (
            "a misspelled setter inside the script",
            String::new(),
            format!("{LOAD}\nfunction clear() {{ setDomians([]); }}"),
        ),
    ];
    for (mistake, markup, script) in cases {
        assert!(refused(&markup, &script), "not refused: {mistake}");
    }
    assert!(
        !refused(&rows("domain.name"), LOAD),
        "correct code is refused"
    );

    // A key written in the call is the compiler's to refuse.
    assert_eq!(
        payload(r#"pp.rpc("rename_domain", { nmae: "x" });"#),
        "written"
    );
}

#[test]
fn mistakes_that_still_compile() {
    let append = format!(
        r#"{LOAD}
        async function add() {{
            const made = await pp.rpc("add_domain", {{}});
            setDomains((current) => [...current, made]);
        }}"#
    );
    let built = r#"
        const [domains, setDomains] = pp.state([]);
        async function load() {
            const list = await pp.rpc("list_domains", {});
            setDomains(list.map((domain) => ({ ...domain, open: false })));
        }
    "#;
    let cases: [(&str, String, String); 5] = [
        // Hooks: `pp` is a name, and what is read off it is not looked at.
        (
            "a hook that does not exist",
            String::new(),
            "const [open, setOpen] = pp.sate(false);".into(),
        ),
        // Fields: the state loses its shape, so no read of it is checked.
        (
            "a row field, after the state is appended to with an updater",
            rows("domain.nmae"),
            append,
        ),
        (
            "a row field, after the response is mapped into the state",
            rows("domain.nmae"),
            built.into(),
        ),
        (
            "a field of a state that starts as an object literal",
            "<input value={form.emial} />".into(),
            r#"const [form, setForm] = pp.state({ name: "", email: "" });"#.into(),
        ),
        (
            "a field read in a function the row is handed to",
            rows("label(domain)"),
            format!("{LOAD}\nfunction label(domain) {{ return domain.nmae; }}"),
        ),
    ];
    for (mistake, markup, script) in cases {
        assert!(!refused(&markup, &script), "now refused: {mistake}");
    }

    // Payloads: only the rpc's name is checked.
    assert_eq!(
        payload(
            r#"async function save(event) {
                await pp.rpc("rename_domain", Object.fromEntries(new FormData(event.currentTarget)));
            }"#
        ),
        "unknown",
        "a form's fields"
    );
    assert_eq!(
        payload(
            r#"async function save() {
                let draft = { nmae: "x" };
                await pp.rpc("rename_domain", draft);
            }"#
        ),
        "unknown",
        "a payload held in a `let`"
    );
}
