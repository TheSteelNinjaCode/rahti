//! The names a block's code reads that nothing supplies.
//!
//! Each name that is reported is paired with the ordinary code that must not
//! be: a name is only a mistake when it is certain nothing gives it.

use super::*;
use crate::parse::parse;
use proc_macro2::TokenStream;
use std::str::FromStr;

const LOAD: &str = r#"
    const [domains, setDomains] = pp.state([]);
    const [open, setOpen] = pp.state(false);
    async function load() {
        setDomains(await pp.rpc("list_domains", {}));
    }
"#;

/// Each unknown name of `src`, as `place name`.
fn unknown_in(src: &str) -> Vec<String> {
    let tokens = TokenStream::from_str(src).expect("test source does not tokenize");
    let parsed = match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    };
    unknown(&parsed.client_code)
        .into_iter()
        .map(|unknown| format!("{} {}", unknown.place, unknown.name))
        .collect()
}

/// Each unknown name of a block with this markup and this script.
fn found(markup: &str, script: &str) -> Vec<String> {
    unknown_in(&format!(
        "<div>{markup}<script>r##\"{script}\"##</script></div>"
    ))
}

#[test]
fn a_name_the_script_does_not_declare_is_unknown() {
    assert_eq!(
        found(r#"<p hidden={domians.length === 0}>"None"</p>"#, LOAD),
        ["binding domians"]
    );
    assert_eq!(
        found(r#"<button onclick={laod()}>"Reload"</button>"#, LOAD),
        ["handler laod"]
    );
    assert_eq!(
        found(
            "",
            &format!("{LOAD}\nfunction clear() {{ setDomians([]); }}")
        ),
        ["script setDomians"]
    );
    assert_eq!(
        found(
            r#"<template pp-for="domain in domains"><li key={domain.id}>{domian.name}</li></template>"#,
            LOAD
        ),
        ["binding domian"]
    );
    // The collection a loop reads is a binding too.
    assert_eq!(
        found(
            r#"<template pp-for="domain in domians"><li key={domain.id}></li></template>"#,
            LOAD
        ),
        ["binding domians"]
    );
}

#[test]
fn what_the_block_supplies_is_known() {
    let markup = r#"
        <p hidden={domains.length === 0 || !open}>{pp.props.title}</p>
        <button onclick={setOpen(!open)}>"Toggle"</button>
        <button onclick={load()}>"Reload"</button>
        <input oninput={setOpen(event.target.value !== "" && e && el && currentTarget)} />
        <template pp-for="(domain, index) in domains.filter((domain) => domain.name)">
            <li key={domain.id} onclick={setOpen(index > 0)}>
                {index}
                <template pp-for="record in domain.records">
                    <span key={record.kind}>{record.value + domain.name}</span>
                </template>
            </li>
        </template>
    "#;
    assert!(found(markup, LOAD).is_empty());
}

#[test]
fn the_window_is_known() {
    let script = r#"
        const [width, setWidth] = pp.state(window.innerWidth);
        pp.effect(() => {
            const timer = setTimeout(() => setWidth(innerWidth), 100);
            const chart = typeof Chart === "undefined" ? null : new Chart(document.body);
            console.log(JSON.stringify(navigator.userAgent), Math.max(1, 2), chart);
            return () => clearTimeout(timer);
        }, []);
    "#;
    let markup = r#"<p class={twMerge("p-2", width > 600 ? "p-4" : "")}>{Math.round(width)}</p>"#;
    assert!(found(markup, script).is_empty());
    // A lower-case name no window has is not one.
    assert_eq!(found("", r#"gtag("event", "load");"#), ["script gtag"]);
    assert!(found("", r#"window.gtag("event", "load");"#).is_empty());
}

#[test]
fn a_block_without_one_script_of_its_own_has_no_names_to_check() {
    // No script: the block renders in the scope of the block around it.
    assert!(unknown_in("<p hidden={!open}>{label}</p>").is_empty());
    // Two scripts: there is no one scope, but each script is still read.
    assert_eq!(
        unknown_in(
            "<div><p>{label}</p>
                <script>r##\"const a = 1;\"##</script>
                <script>r##\"const b = c;\"##</script>
             </div>"
        ),
        ["script c"]
    );
    // Fragment-rooted children read the component they are passed to.
    assert!(
        unknown_in(
            "<div><Card><><p>{theirs}</p><p>{alsoTheirs}</p></></Card>
                <script>r##\"const mine = 1;\"##</script>
             </div>"
        )
        .is_empty()
    );
    // Element-rooted children read this block.
    assert_eq!(
        unknown_in(
            "<div><Card><p>{mine + theirs}</p></Card>
                <script>r##\"const mine = 1;\"##</script>
             </div>"
        ),
        ["binding theirs"]
    );
}

#[test]
fn the_refusal_names_the_nearest_known_name() {
    let known = [
        "domains".to_string(),
        "load".to_string(),
        "open".to_string(),
    ];
    let message = crate::authoring::unknown_name("binding", "domians", &known, "");
    assert!(message.contains("RAHTI_UNKNOWN_NAME"), "{message}");
    assert!(message.contains("Did you mean `domains`?"), "{message}");
    let message = crate::authoring::unknown_name("handler", "refresh", &known, "");
    assert!(!message.contains("Did you mean"), "{message}");
    assert!(message.contains("`domains`, `load`, `open`"), "{message}");
    // A long scope is cut short.
    let many: Vec<String> = (0..20).map(|index| format!("name{index:02}")).collect();
    let message = crate::authoring::unknown_name("binding", "refresh", &many, "");
    assert!(message.contains("`name11`, and 8 more"), "{message}");
}
