//! The checks `html!` writes for each `pp.rpc` call in a template.
//!
//! What the checks then decide — an unknown key, a required parameter left
//! out — is evaluated by rustc against the `#[rpc]`'s signature, and is
//! tested beside those functions in `rahti::rpc_check`. Here: which calls get
//! which checks, which are refused before any, and where each one lands.
//! Each refusal is paired with the ordinary code it must not catch.

use super::*;
use crate::parse::{Parsed, parse};
use proc_macro2::TokenStream;
use std::str::FromStr;

fn parsed(src: &str) -> Parsed {
    let tokens = TokenStream::from_str(src).expect("test source does not tokenize");
    match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    }
}

/// Every check the template's client code gets, as one token string.
fn checked(src: &str) -> String {
    let parsed = parsed(src);
    let mut next = 0;
    let mut out = TokenStream::new();
    for source in &parsed.client_code {
        match checks(source, &mut next) {
            Ok(tokens) => out.extend(tokens),
            Err(err) => panic!("expected `{src}` to be checked, got:\n{}", err.msg),
        }
    }
    out.to_string()
}

fn refused(src: &str) -> String {
    let parsed = parsed(src);
    let mut next = 0;
    for source in &parsed.client_code {
        if let Err(err) = checks(source, &mut next) {
            return err.msg;
        }
    }
    panic!("expected `{src}` to be refused")
}

fn script(body: &str) -> String {
    format!("<div><script>r##\"{body}\"##</script></div>")
}

// ----------------------------------------------------------- what is checked

/// The name resolves to the signature `#[rpc]` wrote; each key is checked,
/// and so is the set of keys, since an object literal is all that is sent.
#[test]
fn an_object_literal_call_checks_its_name_each_key_and_the_set() {
    let out = checked(r#"<button onclick={pp.rpc("rename", { id, name: draft })}>"Go"</button>"#);
    assert!(
        out.contains(
            "const __RAHTI_RPC_CALL_0 : & :: rahti :: rpc_check :: Signature = & __RAHTI_RPC_rename"
        ),
        "{out}"
    );
    assert!(
        out.contains(r#"key (__RAHTI_RPC_CALL_0 , "id" , "")"#),
        "{out}"
    );
    assert!(
        out.contains(r#"key (__RAHTI_RPC_CALL_0 , "name" , "")"#),
        "{out}"
    );
    assert!(
        out.contains(r#"complete (__RAHTI_RPC_CALL_0 , & ["id" , "name"] , "")"#),
        "{out}"
    );
}

/// Rule 50's form submit: the keys are the form's fields, known only in the
/// browser, so the name is checked and the payload is not.
#[test]
fn a_form_data_submit_checks_only_its_name() {
    let out = checked(&script(
        "async function save(event) {\n\
             event.preventDefault();\n\
             await pp.rpc(\"save_contact\", Object.fromEntries(new FormData(event.currentTarget)));\n\
         }",
    ));
    assert!(out.contains("= & __RAHTI_RPC_save_contact"), "{out}");
    assert!(!out.contains("key ("), "{out}");
    assert!(!out.contains("complete ("), "{out}");
}

/// A spread may send any key, so only the keys written are checked.
#[test]
fn a_spread_payload_checks_its_written_keys_but_not_the_set() {
    let out = checked(&script("pp.rpc(\"update\", { ...draft, id });"));
    assert!(out.contains(r#"key (__RAHTI_RPC_CALL_0 , "id""#), "{out}");
    assert!(!out.contains("complete ("), "{out}");
}

/// `pp.rpc("list")` sends `{}`: fine unless a parameter is required.
#[test]
fn a_call_without_a_payload_checks_that_nothing_is_required() {
    let out = checked(r#"<p>{pp.rpc("list")}</p>"#);
    assert!(out.contains("no_payload (__RAHTI_RPC_CALL_0"), "{out}");
}

#[test]
fn literal_values_are_checked_but_dynamic_or_overwritable_values_are_not() {
    let out = checked(&script("pp.rpc(\"rename\", { name: 12, id: currentId });"));
    assert!(out.contains("Literal :: Number"), "{out}");
    assert_eq!(out.matches("rpc_check :: value").count(), 1, "{out}");
    for payload in [
        "{ name: 12, ...draft }",
        "{ ...draft, name: 12 }",
        "payload",
    ] {
        let out = checked(&script(&format!("pp.rpc(\"rename\", {payload});")));
        assert!(!out.contains("rpc_check :: value"), "{out}");
    }
}

/// Calls are numbered across the block, so each has its own constant.
#[test]
fn every_call_gets_its_own_constant() {
    let out = checked(&script("pp.rpc(\"a\", {});\npp.rpc(\"b\", {});"));
    assert!(
        out.contains("__RAHTI_RPC_CALL_0 : & :: rahti :: rpc_check :: Signature = & __RAHTI_RPC_a")
    );
    assert!(
        out.contains("__RAHTI_RPC_CALL_1 : & :: rahti :: rpc_check :: Signature = & __RAHTI_RPC_b")
    );
}

/// In a script the line is part of the message, since the span is the
/// script's; a binding's own span says where it is.
#[test]
fn only_a_script_names_the_line() {
    assert!(checked(&script("pp.rpc(\"a\", { b });")).contains(r#""b" , """#));
}

/// Calls that are not `pp.rpc` are someone else's, and are not checked or
/// refused however they name what they call.
#[test]
fn other_calls_named_rpc_are_left_alone() {
    let out = checked(&script(
        "rpc(name, {});\napi.rpc(name, {});\nclient.pp.rpc(name);\npp[\"rpc\"](name, {});\n\
         pp.socket(\"chat\", { room });",
    ));
    assert!(out.is_empty(), "{out}");
}

// ------------------------------------------------------------------ refusals

/// The name is what is checked, so it has to be written.
#[test]
fn a_name_that_is_not_a_literal_is_refused_with_the_form_to_write() {
    for src in [
        r#"<button onclick={pp.rpc(which, { id })}>"Go"</button>"#.to_string(),
        script("const name = \"save\";\npp.rpc(name, {});"),
        script("pp.rpc(\"save_\" + kind, {});"),
        script("pp.rpc(`save_${kind}`, {});"),
    ] {
        let msg = refused(&src);
        assert!(
            msg.contains("names its `#[rpc]` with a string literal"),
            "{msg}"
        );
        assert!(msg.contains(r#"`pp.rpc("save", { … })`"#), "{msg}");
    }
    let msg = refused(&script("pp.rpc();"));
    assert!(msg.contains("`pp.rpc()` names no `#[rpc]`"), "{msg}");
}

/// A literal written in either quote, or as a plain template, is a literal.
#[test]
fn a_literal_name_in_any_quote_is_not_refused() {
    let out = checked(&script(
        "pp.rpc('a', {});\npp.rpc(`b`, {});\npp.rpc(\"c\", {});",
    ));
    for name in ["a", "b", "c"] {
        assert!(out.contains(&format!("& __RAHTI_RPC_{name}")), "{out}");
    }
}

/// An rpc is a Rust function, so its name is a Rust identifier.
#[test]
fn a_name_no_function_can_have_is_refused() {
    let msg = refused(&script("pp.rpc(\"save-contact\", {});"));
    assert!(
        msg.contains("an rpc's name is its Rust function's name"),
        "{msg}"
    );
}

// ---------------------------------------------------------------- locations

/// In a binding, each check lands on the token it is about: the name's
/// string, a key.
#[test]
fn a_binding_locates_the_name_and_each_key() {
    let parsed = parsed("<p>{pp.rpc(\"rename\", { id, name: 1 })}</p>");
    let source = &parsed.client_code[0];
    let locator = Locator::new(source);
    let calls = rahti_js::rpc_calls(rahti_js::Code::Expression, &source.source);
    let RpcName::Literal { span, .. } = &calls[0].name else {
        panic!("{:?}", calls[0].name);
    };
    // `<p>{pp.rpc(` is 11 characters.
    assert_eq!(locator.span(span).start().column, 11);
    let Payload::Object { keys, span, .. } = &calls[0].payload else {
        panic!("{:?}", calls[0].payload);
    };
    assert_eq!(locator.span(&keys[1].span).start().column, 27);
    assert_eq!(locator.span(span).start().column, 21);
    assert_eq!(locator.span(span).end().column, 36);
}
