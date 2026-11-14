//! The names a piece of client code reads without declaring them.

use crate::{Code, free_names, is_global};

fn free(code: Code, source: &str) -> Vec<String> {
    free_names(code, source)
        .into_iter()
        .map(|(name, span)| {
            assert_eq!(&source[span], name, "the span is where the name is written");
            name
        })
        .collect()
}

#[test]
fn a_binding_reads_the_names_it_does_not_declare() {
    assert_eq!(
        free(Code::Expression, "domains.length > 0 && !open"),
        ["domains", "open"]
    );
    // A callback's parameter is its own, and a property is not a name.
    assert_eq!(
        free(
            Code::Expression,
            "rows.filter((row) => row.open).map(label)"
        ),
        ["rows", "label"]
    );
    // Once, where it is first written.
    assert_eq!(free(Code::Expression, "count + count"), ["count"]);
    assert_eq!(free(Code::Expression, "pp.props.title"), ["pp"]);
}

#[test]
fn a_handler_reads_its_names_as_an_expression_or_a_body() {
    assert_eq!(free(Code::Handler, "setOpen(!open)"), ["setOpen", "open"]);
    assert_eq!(
        free(
            Code::Handler,
            "event.preventDefault(); const next = draft; save(next);"
        ),
        ["event", "draft", "save"]
    );
}

#[test]
fn a_script_is_handed_pp_and_reads_what_it_does_not_declare() {
    let script = r#"
        const [domains, setDomains] = pp.state([]);
        async function load() {
            setDomians(await pp.rpc("list_domains", {}));
            console.log(domains, __pp_server[0]);
        }
    "#;
    assert_eq!(free(Code::Script, script), ["setDomians", "console"]);
    // A function is known above the line that declares it.
    assert!(free(Code::Script, "start(); function start() {}").is_empty());
}

#[test]
fn asking_whether_a_name_exists_does_not_read_it() {
    assert!(free(Code::Script, r#"if (typeof Chart !== "undefined") {}"#).is_empty());
    assert_eq!(
        free(
            Code::Script,
            r#"const chart = typeof Chart === "function" ? make() : null;"#
        ),
        ["make"]
    );
    // Code that does not parse reads nothing: compiling it says why.
    assert!(free(Code::Expression, "a +").is_empty());
}

#[test]
fn the_window_and_capitalized_names_are_global() {
    for name in [
        "window",
        "fetch",
        "setTimeout",
        "Math",
        "GPUBufferUsage",
        "Chart",
    ] {
        assert!(is_global(name), "{name}");
    }
    for name in ["domains", "setOpen", "laod", "gtag"] {
        assert!(!is_global(name), "{name}");
    }
}
