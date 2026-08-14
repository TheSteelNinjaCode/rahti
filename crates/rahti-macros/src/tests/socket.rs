//! What `#[socket]` writes beside the function it is written on.
//!
//! The expansion is compared as a token string, so the spacing is quote's
//! rather than rustfmt's — see the note atop the `#[rpc]` tests.

use super::*;
use proc_macro2::TokenStream;
use std::str::FromStr;

fn ts(src: &str) -> TokenStream {
    TokenStream::from_str(src).expect("test source does not tokenize")
}

fn expanded(item: &str) -> String {
    match expand(TokenStream::new(), ts(item)) {
        Ok(out) => out.to_string(),
        Err(err) => panic!("expected `{item}` to expand, got:\n{}", err.msg),
    }
}

fn expanded_with(attr: &str, item: &str) -> String {
    match expand(ts(attr), ts(item)) {
        Ok(out) => out.to_string(),
        Err(err) => panic!("expected `#[socket({attr})]` to expand, got:\n{}", err.msg),
    }
}

fn error(item: &str) -> String {
    error_with("", item)
}

fn error_with(attr: &str, item: &str) -> String {
    match expand(ts(attr), ts(item)) {
        Ok(out) => panic!("expected a failure, it expanded to:\n{out}"),
        Err(err) => err.msg,
    }
}

// ------------------------------------------------------------------- shape

#[test]
fn the_function_is_left_alone() {
    let out = expanded("pub async fn chat(room: String, mut socket: Socket) { todo!() }");
    assert!(out.contains("pub async fn chat"), "{out}");
    assert!(out.contains("room : String"), "{out}");
}

#[test]
fn the_shim_is_named_after_the_function() {
    let out = expanded("pub async fn chat(room: String, mut socket: Socket) { todo!() }");
    assert!(out.contains("pub async fn __rahti_socket_chat"), "{out}");
    assert!(out.contains("__socket : :: rahti :: ws :: Socket"), "{out}");
}

#[test]
fn arguments_are_read_by_name_and_the_socket_is_passed_last() {
    let out =
        expanded("pub async fn chat(room: String, limit: u32, mut socket: Socket) { todo!() }");
    assert!(
        out.contains("arg :: < String > (& __args , \"chat\" , \"room\")"),
        "{out}"
    );
    assert!(
        out.contains("arg :: < u32 > (& __args , \"chat\" , \"limit\")"),
        "{out}"
    );
    assert!(out.contains("chat (room , limit , __socket)"), "{out}");
}

#[test]
fn a_socket_can_take_no_arguments() {
    let out = expanded("pub async fn feed(mut socket: Socket) { todo!() }");
    assert!(out.contains("feed (__socket)"), "{out}");
}

#[test]
fn an_unread_argument_refuses_the_connection() {
    let out = expanded("pub async fn chat(room: String, mut socket: Socket) { todo!() }");
    assert!(out.contains("ws :: refuse (__socket , message)"), "{out}");
}

#[test]
fn the_entry_is_registered() {
    let out = expanded("pub async fn chat(mut socket: Socket) { todo!() }");
    assert!(out.contains("inventory :: submit !"), "{out}");
    assert!(out.contains("SocketEntry"), "{out}");
    assert!(out.contains("name : \"chat\""), "{out}");
    assert!(out.contains("auth : false"), "{out}");
}

// ------------------------------------------------------------------ result

#[test]
fn a_fallible_socket_keeps_a_sender_for_the_error_frame() {
    let out = expanded("pub async fn chat(mut socket: Socket) -> Result<()> { todo!() }");
    assert!(out.contains("let __sender = __socket . sender ()"), "{out}");
    assert!(out.contains("ws :: failed (& __sender , error)"), "{out}");
}

#[test]
fn an_infallible_socket_just_runs() {
    let out = expanded("pub async fn chat(mut socket: Socket) { todo!() }");
    assert!(!out.contains("__sender"), "{out}");
}

#[test]
fn a_qualified_result_counts() {
    let out = expanded("pub async fn chat(mut socket: Socket) -> rahti::Result<()> { todo!() }");
    assert!(out.contains("ws :: failed"), "{out}");
}

#[test]
fn a_returned_value_is_refused() {
    let msg = error("pub async fn chat(mut socket: Socket) -> String { todo!() }");
    assert!(msg.contains("answers through its `Socket`"), "{msg}");

    let msg = error("pub async fn chat(mut socket: Socket) -> Result<String> { todo!() }");
    assert!(msg.contains("answers through its `Socket`"), "{msg}");
}

#[test]
fn a_unit_return_is_fine() {
    let out = expanded("pub async fn chat(mut socket: Socket) -> () { todo!() }");
    assert!(out.contains("chat (__socket)"), "{out}");
}

// -------------------------------------------------------------------- auth

#[test]
fn auth_marks_the_entry() {
    let out = expanded_with("auth", "pub async fn feed(mut socket: Socket) { todo!() }");
    assert!(out.contains("auth : true"), "{out}");
}

#[test]
fn other_attribute_arguments_are_refused() {
    let msg = error_with("admin", "pub async fn feed(mut socket: Socket) { todo!() }");
    assert!(msg.contains("takes `auth`, or nothing"), "{msg}");
}

// ------------------------------------------------------------------- rules

#[test]
fn a_socket_must_be_async() {
    let msg = error("pub fn chat(mut socket: Socket) { todo!() }");
    assert!(msg.contains("not `async`"), "{msg}");
}

#[test]
fn a_socket_parameter_is_required() {
    let msg = error("pub async fn chat(room: String) { todo!() }");
    assert!(msg.contains("takes no `Socket`"), "{msg}");
}

#[test]
fn the_socket_must_be_last() {
    let msg = error("pub async fn chat(mut socket: Socket, room: String) { todo!() }");
    assert!(msg.contains("has to be last"), "{msg}");
}

#[test]
fn two_sockets_are_refused() {
    let msg = error("pub async fn chat(a: Socket, b: Socket) { todo!() }");
    assert!(msg.contains("two `Socket` parameters"), "{msg}");
}

#[test]
fn a_borrowed_argument_is_refused() {
    let msg = error("pub async fn chat(room: &str, mut socket: Socket) { todo!() }");
    assert!(msg.contains("borrowed"), "{msg}");
}

#[test]
fn a_method_is_refused() {
    let msg = error("pub async fn chat(self, mut socket: Socket) { todo!() }");
    assert!(msg.contains("belongs to a type"), "{msg}");
}

#[test]
fn a_generic_socket_is_refused() {
    let msg = error("pub async fn chat<T>(value: T, mut socket: Socket) { todo!() }");
    assert!(msg.contains("generic"), "{msg}");
}

#[test]
fn not_a_function_is_refused() {
    let msg = error("struct Chat;");
    assert!(msg.contains("goes on a function"), "{msg}");
}
