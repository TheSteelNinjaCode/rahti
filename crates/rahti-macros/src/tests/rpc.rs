//! What `#[rpc]` writes beside the function it is written on.
//!
//! The expansion is compared as a token string, so the spacing is quote's
//! rather than rustfmt's — `arg :: < String >` is what `arg::<String>()`
//! looks like here.

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

/// The same, with the attribute's own arguments — `#[rpc(auth)]`.
fn expanded_with(attr: &str, item: &str) -> String {
    match expand(ts(attr), ts(item)) {
        Ok(out) => out.to_string(),
        Err(err) => panic!("expected `#[rpc({attr})]` to expand, got:\n{}", err.msg),
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

/// The function is the author's. The attribute only adds to it.
#[test]
fn the_function_is_left_alone() {
    let out = expanded("pub async fn greet(name: String) -> Greeting { todo!() }");
    assert!(out.contains("pub async fn greet"), "{out}");
    assert!(out.contains("name : String"), "{out}");
    assert!(out.contains("-> Greeting"), "{out}");
}

/// The name the dispatcher calls. Generated code is written from the same
/// rule in `rahti-build`, so the two have to agree exactly.
#[test]
fn the_shim_is_named_after_the_function() {
    let out = expanded("pub async fn greet(name: String) -> Greeting { todo!() }");
    assert!(out.contains("pub async fn __rahti_rpc_greet"), "{out}");
    // The request, not a finished call: how much of the body to read is the
    // shim's own decision.
    //
    // Reached through `::rahti::`, not as a bare `axum::`: a component library
    // depends on `rahti` alone, and an attribute this crate provides must not
    // force axum into that library's manifest.
    assert!(
        out.contains("__req : :: rahti :: axum :: extract :: Request"),
        "{out}"
    );
    assert!(out.contains("-> :: rahti :: RpcResponse"), "{out}");
    assert!(out.contains("RpcCall :: from_request (__req)"), "{out}");
}

// --------------------------------------------------------------- arguments

/// The payload names its arguments, and the name it uses is the parameter's.
#[test]
fn every_argument_is_read_by_its_own_name() {
    let out = expanded("pub async fn divide(a: f64, b: f64) -> f64 { todo!() }");
    assert!(out.contains("arg :: < f64 > (\"a\")"), "{out}");
    assert!(out.contains("arg :: < f64 > (\"b\")"), "{out}");
    assert!(out.contains("divide (a , b)"), "{out}");
}

/// A generic type is one type, commas and all.
#[test]
fn a_generic_argument_stays_whole() {
    let out = expanded(
        "pub async fn save(rows: Vec<String>, index: HashMap<String, u32>) -> bool { todo!() }",
    );
    // Two closing angles come back joined, exactly as `Vec<String>>` parses.
    assert!(out.contains("arg :: < Vec < String >> (\"rows\")"), "{out}");
    assert!(
        out.contains("arg :: < HashMap < String , u32 > > (\"index\")"),
        "{out}"
    );
}

#[test]
fn a_call_can_take_no_arguments() {
    let out = expanded("pub async fn ping() -> bool { todo!() }");
    assert!(out.contains("ping ()"), "{out}");
    assert!(!out.contains("arg ::"), "{out}");
}

/// The value is built from the payload rather than pointed at it, and the
/// bound that says so would report against generated code.
#[test]
fn a_borrowed_argument_is_refused() {
    let msg = error("pub async fn greet(name: &str) -> String { todo!() }");
    assert!(msg.contains("borrowed"), "{msg}");
    assert!(msg.contains("`String` rather than `&str`"), "{msg}");
}

#[test]
fn an_argument_without_a_name_is_refused() {
    let msg = error("pub async fn greet((a, b): (u8, u8)) -> String { todo!() }");
    assert!(msg.contains("needs a plain name"), "{msg}");
}

#[test]
fn a_method_is_refused() {
    let msg = error("pub async fn greet(self, name: String) -> String { todo!() }");
    assert!(msg.contains("takes `self`"), "{msg}");
}

// ------------------------------------------------------------------ result

/// `rahti::Error` is not `Serialize` and a blanket impl would overlap, so
/// which form the function returns is read off the return type here.
#[test]
fn an_infallible_return_is_serialized_as_is() {
    let out = expanded("pub async fn greet(name: String) -> Greeting { todo!() }");
    assert!(
        out.contains(":: rahti :: rpc_ok (& (greet (name) . await))"),
        "{out}"
    );
    assert!(!out.contains("rpc_failed"), "{out}");
}

#[test]
fn a_result_is_unwrapped_into_a_value_or_a_failure() {
    let out = expanded("pub async fn divide(a: f64, b: f64) -> Result<f64> { todo!() }");
    assert!(out.contains("rpc_ok (& (value))"), "{out}");
    assert!(out.contains("rpc_failed (error)"), "{out}");
}

/// The name that matters is the last one before the generics.
#[test]
fn a_qualified_result_counts_as_one() {
    let out = expanded("pub async fn load() -> rahti::Result<Vec<Row>> { todo!() }");
    assert!(out.contains("rpc_failed"), "{out}");
}

/// `Vec<Result<…>>` is a value, not a failure.
#[test]
fn a_result_inside_another_type_does_not_count() {
    let out = expanded("pub async fn load() -> Vec<Result<Row>> { todo!() }");
    assert!(!out.contains("rpc_failed"), "{out}");
}

#[test]
fn a_where_clause_does_not_hide_the_return_type() {
    let out = expanded("pub async fn load() -> Result<Row> where Row: Clone { todo!() }");
    assert!(out.contains("rpc_failed"), "{out}");
}

#[test]
fn a_function_returning_nothing_is_refused() {
    let msg = error("pub async fn ping() { todo!() }");
    assert!(msg.contains("returns nothing"), "{msg}");
}

// ------------------------------------------------------------------- calls

/// A blocking rpc is ordinary: not everything worth calling is `async`.
#[test]
fn a_synchronous_function_is_called_without_await() {
    let out = expanded("pub fn ping() -> bool { true }");
    assert!(out.contains("rpc_ok (& (ping ()))"), "{out}");
    assert!(!out.contains("ping () . await"), "{out}");
}

// ------------------------------------------------------------------ misuse

#[test]
fn the_attribute_takes_auth_or_nothing() {
    for attr in ["\"greet\"", "auth = true", "authed", "auth, auth"] {
        let msg = error_with(attr, "pub async fn greet() -> String { todo!() }");
        assert!(msg.contains("takes `auth`, or nothing"), "{attr}: {msg}");
    }
}

// -------------------------------------------------------------------- auth

/// `#[rpc(auth)]` refuses the call before it reads the payload: parsing a body
/// for a caller who has no session is work done for nobody.
#[test]
fn the_auth_form_checks_for_a_session_first() {
    let out = expanded_with(
        "auth",
        "pub async fn rename(name: String) -> Account { todo!() }",
    );

    let check = out.find("auth :: require").expect("the session check");
    let read = out.find("RpcCall :: from_request").expect("the body read");
    assert!(check < read, "the body is read before the check:\n{out}");
    assert!(out.contains("return response"), "{out}");
}

/// And a plain `#[rpc]` has none of it: an open call pays nothing for a
/// feature it did not ask for.
#[test]
fn a_plain_rpc_carries_no_session_check() {
    let out = expanded("pub async fn greet(name: String) -> String { todo!() }");
    assert!(!out.contains("auth :: require"), "{out}");
}

/// Whatever else `auth` changes, it does not change the name the browser calls
/// or the registry entry the linker collects.
#[test]
fn the_auth_form_registers_under_the_same_name() {
    let out = expanded_with(
        "auth",
        "pub async fn rename(name: String) -> Account { todo!() }",
    );
    assert!(out.contains("name : \"rename\""), "{out}");
    assert!(out.contains("pub async fn __rahti_rpc_rename"), "{out}");
}

#[test]
fn something_that_is_not_a_function_is_refused() {
    let msg = error("struct Greeting { message: String }");
    assert!(msg.contains("goes on a function"), "{msg}");
}

/// One payload, one signature: there is no type for a caller to name.
#[test]
fn a_generic_function_is_refused() {
    let msg = error("pub async fn greet<T: Into<String>>(name: T) -> String { todo!() }");
    assert!(msg.contains("cannot be a generic"), "{msg}");
}

// ------------------------------------------------------------------ files

/// An upload is not a value in the payload: `RpcClient` switches to
/// `FormData` when a `File` is in it, so the parameter is read from the parts.
#[test]
fn a_file_parameter_is_taken_from_the_upload() {
    let out = expanded("pub async fn receive(upload: RpcFile) -> bool { todo!() }");
    assert!(out.contains("__call . file (\"upload\")"), "{out}");
    assert!(!out.contains("arg ::"), "{out}");
}

#[test]
fn an_optional_file_may_be_left_out() {
    let out = expanded("pub async fn receive(upload: Option<RpcFile>) -> bool { todo!() }");
    assert!(out.contains("__call . optional_file (\"upload\")"), "{out}");
}

/// A `FileList` sends several parts under one name.
#[test]
fn a_list_of_files_is_read_as_a_list() {
    let out = expanded("pub async fn receive(uploads: Vec<RpcFile>) -> bool { todo!() }");
    assert!(out.contains("__call . files (\"uploads\")"), "{out}");
}

/// The values beside a file are still values — `FormData` carries them as
/// strings, and the runtime reads them back.
#[test]
fn a_value_beside_a_file_is_still_read_from_the_payload() {
    let out = expanded("pub async fn receive(note: String, upload: RpcFile) -> bool { todo!() }");
    assert!(out.contains("arg :: < String > (\"note\")"), "{out}");
    assert!(out.contains("__call . file (\"upload\")"), "{out}");
}

/// Taking a file out of the call mutates it; asking for a value does not, and
/// a needless `mut` is a warning against code nobody wrote.
#[test]
fn only_a_call_that_takes_files_is_binding_mutably() {
    let files = expanded("pub async fn receive(upload: RpcFile) -> bool { todo!() }");
    assert!(files.contains("mut __call"), "{files}");

    let values = expanded("pub async fn greet(name: String) -> String { todo!() }");
    assert!(!values.contains("mut __call"), "{values}");
}

// ---------------------------------------------------------------- streaming

/// An `RpcUpload` changes how the whole call is read: the body is not taken
/// off the wire first, it is left there for the handler.
#[test]
fn an_upload_parameter_reads_the_call_as_a_stream() {
    let out = expanded("pub async fn receive(upload: RpcUpload) -> bool { todo!() }");
    assert!(
        out.contains("from_request_streaming (__req , \"upload\")"),
        "{out}"
    );
    assert!(out.contains("__call . upload (\"upload\")"), "{out}");
    assert!(!out.contains("from_request (__req)"), "{out}");
}

/// The arguments beside it are the ones that arrived in front of it, and they
/// are read the ordinary way.
#[test]
fn a_value_beside_a_streamed_file_is_still_read_from_the_payload() {
    let out = expanded("pub async fn receive(note: String, upload: RpcUpload) -> bool { todo!() }");
    assert!(out.contains("arg :: < String > (\"note\")"), "{out}");
    assert!(out.contains("__call . upload (\"upload\")"), "{out}");
}

/// Multipart is sequential, so two streams would each have to be read before
/// the other.
#[test]
fn more_than_one_upload_is_refused() {
    let msg = error("pub async fn receive(a: RpcUpload, b: RpcUpload) -> bool { todo!() }");
    assert!(msg.contains("more than one `RpcUpload`"), "{msg}");
    assert!(
        msg.contains("RpcFile"),
        "the message says what to use instead: {msg}"
    );
}

/// Nothing can be read after the stream, so a parameter written after it
/// would be a parameter the signature promises and the wire cannot deliver.
#[test]
fn an_upload_that_is_not_last_is_refused() {
    let msg = error("pub async fn receive(upload: RpcUpload, note: String) -> bool { todo!() }");
    assert!(msg.contains("has to be the last one"), "{msg}");
    assert!(
        msg.contains("input.files[0]"),
        "the message shows the payload: {msg}"
    );
}

/// Last is last, whatever else is in front of it.
#[test]
fn an_upload_after_every_other_parameter_is_accepted() {
    let out = expanded(
        "pub async fn receive(note: String, tags: Vec<String>, upload: RpcUpload) -> bool { todo!() }",
    );
    assert!(out.contains("__call . upload (\"upload\")"), "{out}");
}

/// `Vec<String>` is a value. Only the file types are read from the parts.
#[test]
fn a_generic_that_is_not_a_file_is_a_value() {
    let out =
        expanded("pub async fn save(rows: Vec<String>, when: Option<u64>) -> bool { todo!() }");
    assert!(out.contains("arg :: < Vec < String >> (\"rows\")"), "{out}");
    assert!(
        out.contains("arg :: < Option < u64 > > (\"when\")"),
        "{out}"
    );
    assert!(!out.contains("__call . file"), "{out}");
}

// ----------------------------------------------------------------- streams

/// The answer arrives in pieces, so it is written as a stream rather than
/// serialized whole.
#[test]
fn a_stream_is_sent_as_a_stream() {
    let out = expanded("pub async fn count(to: usize) -> RpcStream { todo!() }");
    assert!(
        out.contains(":: rahti :: rpc_stream (count (to) . await)"),
        "{out}"
    );
    assert!(!out.contains("rpc_ok"), "{out}");
}

/// A stream that may fail before it starts: the `Err` is an ordinary failure,
/// the `Ok` is the stream.
#[test]
fn a_fallible_stream_streams_its_ok_value() {
    let out = expanded("pub async fn count(to: usize) -> Result<RpcStream> { todo!() }");
    assert!(out.contains(":: rahti :: rpc_stream (value)"), "{out}");
    assert!(out.contains("rpc_failed (error)"), "{out}");
}

#[test]
fn a_qualified_stream_counts_as_one() {
    let out = expanded("pub async fn count() -> rahti::RpcStream { todo!() }");
    assert!(out.contains("rpc_stream"), "{out}");
}

/// Anything else is a value, streams inside it or not.
#[test]
fn a_value_is_not_a_stream() {
    let out = expanded("pub async fn count() -> Vec<RpcStream> { todo!() }");
    assert!(out.contains("rpc_ok"), "{out}");
    assert!(!out.contains("rpc_stream"), "{out}");
}
