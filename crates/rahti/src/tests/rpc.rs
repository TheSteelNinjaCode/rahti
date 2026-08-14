//! The rpc wire: what a call has to carry, and what comes back.

use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use axum::response::IntoResponse;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A call the way `RpcClient` sends one: the function in a header, the
/// arguments as JSON, and the CSRF token in both a cookie and a header.
fn call(name: &str, payload: &str) -> Request<Body> {
    signed(name, payload, TOKEN, TOKEN)
}

fn signed(name: &str, payload: &str, cookie: &str, header: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/greeter")
        .header("host", "127.0.0.1:3000")
        .header("x-pp-function", name)
        .header("content-type", "application/json")
        .header("cookie", format!("pp_csrf_3000={cookie}; other=x"))
        .header("x-csrf-token", header)
        .body(Body::from(payload.to_string()))
        .expect("a request")
}

async fn body_of(res: Response) -> String {
    let bytes = to_bytes(res.into_body(), 16 << 20).await.expect("a body");
    String::from_utf8(bytes.into()).expect("utf-8")
}

async fn parsed(req: Request<Body>) -> RpcCall {
    match RpcCall::from_request(req).await {
        Ok(call) => call,
        Err(res) => panic!("expected a call, got {}", res.status()),
    }
}

/// The response a refused call comes back as.
async fn refused(req: Request<Body>) -> (StatusCode, String) {
    match RpcCall::from_request(req).await {
        Ok(call) => panic!("expected a refusal, got a call to `{}`", call.name()),
        Err(res) => {
            let status = res.status();
            (status, body_of(res).await)
        }
    }
}

// -------------------------------------------------------------- the call

#[tokio::test]
async fn a_call_carries_its_name_and_its_arguments() {
    let call = parsed(call("greet", r#"{"name":"Ada","times":2}"#)).await;
    assert_eq!(call.name(), "greet");
    assert_eq!(call.arg::<String>("name").ok(), Some("Ada".to_string()));
    assert_eq!(call.arg::<u32>("times").ok(), Some(2));
}

/// Serde's rule for a struct field, applied to an argument: a missing key is
/// `null`, which an `Option` accepts and nothing else does.
#[tokio::test]
async fn a_missing_argument_is_null() {
    let call = parsed(call("greet", r#"{}"#)).await;
    assert_eq!(call.arg::<Option<String>>("name").ok(), Some(None));
    assert!(call.arg::<String>("name").is_err());
}

#[tokio::test]
async fn an_argument_of_the_wrong_type_is_reported_against_its_name() {
    let call = parsed(call("greet", r#"{"name":7}"#)).await;
    let res = call
        .arg::<String>("name")
        .expect_err("a number is not a string");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_of(res).await;
    assert!(body.contains("`name` argument"), "{body}");
    assert!(body.contains("\"error\""), "{body}");
}

/// `pp.rpc("reload")` sends `{}`, but a call with nothing to say need not
/// send a body at all.
#[tokio::test]
async fn an_empty_body_is_a_call_with_no_arguments() {
    let call = parsed(call("reload", "")).await;
    assert_eq!(call.name(), "reload");
    assert!(call.args().is_empty());
}

#[tokio::test]
async fn a_payload_that_is_not_an_object_is_refused() {
    let (status, body) = refused(call("greet", "[1, 2]")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("not a JSON object"), "{body}");
}

#[tokio::test]
async fn a_post_that_names_no_function_is_refused() {
    let req = Request::builder()
        .method("POST")
        .uri("/greeter")
        .header("cookie", format!("pp_csrf={TOKEN}"))
        .header("x-csrf-token", TOKEN)
        .body(Body::empty())
        .expect("a request");
    let (status, body) = refused(req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("named no function"), "{body}");
}

/// A multipart body the parser cannot make sense of is refused with what
/// went wrong, rather than read as a call with no arguments.
#[tokio::test]
async fn an_upload_that_is_not_readable_is_refused() {
    let req = Request::builder()
        .method("POST")
        .uri("/uploader")
        .header("x-pp-function", "receive")
        // No boundary, so there is nothing to split the parts on.
        .header("content-type", "multipart/form-data")
        .header("cookie", format!("pp_csrf={TOKEN}"))
        .header("x-csrf-token", TOKEN)
        .body(Body::from("--x--"))
        .expect("a request");
    let (status, body) = refused(req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("not readable"), "{body}");
}

// -------------------------------------------------------------------- csrf

/// The whole mechanism: a page from another origin carries the cookie but
/// cannot read it, so it cannot produce the header.
#[tokio::test]
async fn a_call_without_the_header_is_refused() {
    let (status, body) = refused(signed("greet", "{}", TOKEN, "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("CSRF"), "{body}");
}

#[tokio::test]
async fn a_call_whose_token_does_not_match_is_refused() {
    let (status, _) = refused(signed("greet", "{}", TOKEN, "something-else")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// An empty cookie is not a token two empty strings can agree on.
#[tokio::test]
async fn an_empty_token_is_not_a_match() {
    let (status, _) = refused(signed("greet", "{}", "", "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// The client prefers `pp_csrf_<port>` when the page has one in its URL, so
/// two apps on one host do not overwrite each other's token.
#[tokio::test]
async fn the_cookie_is_named_after_the_port() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "127.0.0.1:3000".parse().unwrap());
    assert_eq!(cookie_name(&headers), "pp_csrf_3000");

    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "example.com".parse().unwrap());
    assert_eq!(cookie_name(&headers), "pp_csrf");
}

#[test]
fn a_cookie_is_read_out_of_the_header_by_name() {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "a=1; pp_csrf=abc; b=2".parse().unwrap());
    assert_eq!(cookie(&headers, "pp_csrf"), Some("abc"));
    assert_eq!(cookie(&headers, "pp_csrf_3000"), None);
    // A name that is only a prefix of another is not that other one.
    assert_eq!(cookie(&headers, "pp"), None);
}

/// Guessable is not protected, so the bytes come from the operating system.
#[test]
fn a_token_is_random_hex() {
    let first = new_token().expect("the os has entropy");
    let second = new_token().expect("the os has entropy");
    assert_eq!(first.len(), 32);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");
    assert_ne!(first, second);
}

// ---------------------------------------------------------------- answers

#[tokio::test]
async fn a_value_comes_back_as_json() {
    let res = rpc_ok(&serde_json::json!({ "message": "Hello, Ada!" }));
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("application/json")
    );
    assert!(
        res.extensions().get::<RpcBody>().is_some(),
        "an rpc answer is marked"
    );
    assert_eq!(body_of(res).await, r#"{"message":"Hello, Ada!"}"#);
}

/// The status and the message are the function's own, and `{"error": …}` is
/// the field the client reads before it throws.
#[tokio::test]
async fn a_failure_comes_back_as_its_status_and_its_message() {
    let res = rpc_failed(Error::new(StatusCode::NOT_FOUND, "no such customer"));
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_of(res).await, r#"{"error":"no such customer"}"#);
}

#[tokio::test]
async fn an_unknown_name_says_what_would_have_answered_it() {
    let res = rpc_unknown("grret");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = body_of(res).await;
    assert!(body.contains("no rpc named `grret`"), "{body}");
    assert!(body.contains("#[rpc]"), "{body}");
}

/// The caller is `fetch`, so even a panic has to be something it can read.
#[tokio::test]
async fn a_panic_comes_back_as_json() {
    let res = rpc_panic_response(Box::new("everything is fine"));
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(res.extensions().get::<RpcBody>().is_some());
    assert!(body_of(res).await.contains("everything is fine"));
}

/// An error boundary would replace the JSON with a page. It leaves an rpc
/// answer alone — the marker is what tells it apart from a page that failed.
#[tokio::test]
async fn a_boundary_leaves_an_rpc_failure_alone() {
    fn boundary(_: crate::ErrorInfo) -> crate::Html {
        crate::Html::from_raw("<p>sorry</p>".to_string())
    }

    let res = crate::apply_error(boundary, rpc_failed(Error::internal("no"))).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body_of(res).await, r#"{"error":"no"}"#);
}

// ------------------------------------------------------------------ layer

/// A tower layer needs a service under it; this is the smallest one that
/// answers with the response a test wants to see leave the layer.
async fn through_csrf(request: Request<Body>, answer: fn() -> Response) -> Response {
    use axum::Router;
    use axum::routing::get;
    use tower::ServiceExt;

    Router::new()
        .route("/", get(move || async move { answer() }))
        .layer(axum::middleware::from_fn(csrf))
        .oneshot(request)
        .await
        .expect("the router answered")
}

fn page() -> Response {
    crate::Html::from_raw("<p>hello</p>".to_string()).into_response()
}

fn asset() -> Response {
    "body { color: red }".into_response()
}

fn visit(cookie: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().uri("/").header("host", "127.0.0.1:3000");
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    req.body(Body::empty()).expect("a request")
}

/// A page is where a session starts, so that is where the token is set —
/// readable, because the client has to send it back.
#[tokio::test]
async fn a_page_is_handed_a_token() {
    let res = through_csrf(visit(None), page).await;
    let set = res
        .headers()
        .get(header::SET_COOKIE)
        .expect("a page hands out a token")
        .to_str()
        .unwrap()
        .to_string();

    assert!(set.starts_with("pp_csrf_3000="), "{set}");
    assert!(set.contains("Path=/"), "{set}");
    assert!(set.contains("SameSite=Lax"), "{set}");
    assert!(!set.contains("HttpOnly"), "the client reads it: {set}");
}

/// Re-issuing on every response would race the calls already in flight.
#[tokio::test]
async fn a_visitor_who_has_one_keeps_it() {
    let res = through_csrf(visit(Some(&format!("pp_csrf_3000={TOKEN}"))), page).await;
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn a_stylesheet_is_not_where_a_session_starts() {
    let res = through_csrf(visit(None), asset).await;
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

// ------------------------------------------------------------------ upload

const BOUNDARY: &str = "----rahtitest";

/// A multipart body the way `RpcClient.createFormData` builds one: a part per
/// value, and a filename on the parts that are files.
fn form(parts: &[(&str, Option<&str>, &str)]) -> String {
    let mut body = String::new();
    for (name, file_name, value) in parts {
        body.push_str(&format!("--{BOUNDARY}\r\n"));
        match file_name {
            Some(file_name) => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\
                 Content-Type: text/plain\r\n\r\n"
            )),
            None => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"
            )),
        }
        body.push_str(value);
        body.push_str("\r\n");
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    body
}

fn upload(name: &str, parts: &[(&str, Option<&str>, &str)]) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/uploader")
        .header("host", "127.0.0.1:3000")
        .header("x-pp-function", name)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header("cookie", format!("pp_csrf_3000={TOKEN}"))
        .header("x-csrf-token", TOKEN)
        .body(Body::from(form(parts)))
        .expect("a request")
}

#[tokio::test]
async fn a_file_arrives_with_its_name_and_its_bytes() {
    let mut call = parsed(upload(
        "receive",
        &[("upload", Some("notes.txt"), "hello from a file")],
    ))
    .await;

    let file = call.file("upload").expect("the file is there");
    assert_eq!(file.name(), "notes.txt");
    assert_eq!(
        file.text().await.expect("readable"),
        Some("hello from a file".to_string())
    );
    assert_eq!(file.len(), 17);
    assert_eq!(file.content_type(), Some("text/plain"));
    // Well under the threshold, so nothing was written to disk for it.
    assert!(!file.is_spilled());
}

/// A `FileList` is several parts under one name.
#[tokio::test]
async fn every_file_under_one_name_is_kept() {
    let mut call = parsed(upload(
        "receive_all",
        &[
            ("uploads", Some("a.txt"), "one"),
            ("uploads", Some("b.txt"), "two"),
        ],
    ))
    .await;

    let files = call.files("uploads").expect("both files");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name(), "a.txt");
    assert_eq!(
        files[1].text().await.expect("readable"),
        Some("two".to_string())
    );
    // Taken out of the call: asking twice is asking for what is gone.
    assert!(call.files("uploads").expect("no files left").is_empty());
}

/// `FormData` has no types — every value leaves the browser as a string, and
/// a parameter that is not a `String` reads it back.
#[tokio::test]
async fn a_value_beside_a_file_is_read_back_into_its_type() {
    let call = parsed(upload(
        "receive",
        &[
            ("note", None, "from the test"),
            ("count", None, "42"),
            ("flags", None, r#"{"draft":true}"#),
            ("upload", Some("a.txt"), "x"),
        ],
    ))
    .await;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Flags {
        draft: bool,
    }

    assert_eq!(
        call.arg::<String>("note").ok(),
        Some("from the test".to_string())
    );
    assert_eq!(call.arg::<u32>("count").ok(), Some(42));
    // An object leaves the browser as the text of its JSON.
    assert_eq!(call.arg::<Flags>("flags").ok(), Some(Flags { draft: true }));

    // `Value` is the exception, and the only one: a string already *is* a
    // `Value`, so there is nothing that does not fit and nothing to read back.
    assert_eq!(
        call.arg::<serde_json::Value>("count").ok(),
        Some(serde_json::json!("42"))
    );
}

/// The characters the browser sent, even when they look like something else.
#[tokio::test]
async fn a_string_that_looks_like_a_number_stays_a_string() {
    let call = parsed(upload("receive", &[("note", None, "42")])).await;
    assert_eq!(call.arg::<String>("note").ok(), Some("42".to_string()));
}

#[tokio::test]
async fn a_missing_file_is_reported_against_its_name() {
    let mut call = parsed(upload("receive", &[("note", None, "no file")])).await;

    let res = call.file("upload").expect_err("nothing was uploaded");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_of(res).await;
    assert!(body.contains("without its `upload` file"), "{body}");

    assert!(
        call.optional_file("upload")
            .expect("optional is fine")
            .is_none()
    );
}

/// An upload names itself, and the name is whatever the browser was told.
/// Joining that onto a directory unfiltered is how a file lands elsewhere.
#[test]
fn a_reported_name_is_reduced_to_something_safe() {
    let file = |name: &str| RpcFile::from_bytes(name, None, "x");

    assert_eq!(file("notes.txt").safe_name().as_deref(), Some("notes.txt"));
    assert_eq!(
        file("../../etc/passwd").safe_name().as_deref(),
        Some("passwd")
    );
    assert_eq!(
        file(r"C:\Windows\system.ini").safe_name().as_deref(),
        Some("system.ini")
    );
    assert_eq!(
        file("my report (1).pdf").safe_name().as_deref(),
        Some("my_report__1_.pdf")
    );
    assert_eq!(file("..").safe_name(), None);
    assert_eq!(file("").safe_name(), None);
}

/// Against limits a test can reach, so the spill can be asserted on the wire
/// rather than only on the spool underneath it.
async fn parsed_within(req: Request<Body>, limits: Limits) -> RpcCall {
    let (parts, body) = req.into_parts();
    let name = header_str(&parts.headers, "x-pp-function")
        .unwrap_or_default()
        .to_string();
    match RpcCall::from_multipart_within(name, Request::from_parts(parts, body), limits).await {
        Ok(call) => call,
        Err(res) => panic!("expected a call, got {}", res.status()),
    }
}

/// The whole point, seen from the wire: a file large enough to matter is
/// never held, and the handler cannot tell.
#[tokio::test]
async fn a_large_file_spills_on_its_way_in() {
    let big = "x".repeat(4096);
    let limits = Limits {
        file: 1 << 20,
        spill_at: 64,
        value: MAX_BODY,
    };
    let mut call = parsed_within(
        upload(
            "receive",
            &[("note", None, "hi"), ("upload", Some("big.bin"), &big)],
        ),
        limits,
    )
    .await;

    // The value beside it is small and stays a string, spill or no spill.
    assert_eq!(call.arg::<String>("note").ok(), Some("hi".to_string()));

    let file = call.file("upload").expect("the file is there");
    assert!(
        file.is_spilled(),
        "a 4 KiB file past a 64 byte threshold is on disk"
    );
    assert_eq!(file.len(), 4096);
    assert_eq!(file.text().await.expect("readable"), Some(big));
}

/// The body limit is shared, so without a per-file cap one part of a
/// `FileList` can spend the whole allowance.
#[tokio::test]
async fn a_file_over_the_per_file_limit_is_refused() {
    let limits = Limits {
        file: 8,
        spill_at: 4,
        value: MAX_BODY,
    };
    let req = upload(
        "receive",
        &[("upload", Some("big.bin"), "far more than eight bytes")],
    );
    let (parts, body) = req.into_parts();

    let res = RpcCall::from_multipart_within(
        "receive".to_string(),
        Request::from_parts(parts, body),
        limits,
    )
    .await
    .expect_err("over the limit");

    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = body_of(res).await;
    assert!(body.contains("larger than"), "{body}");
    assert!(
        body.contains(MAX_FILE_ENV),
        "the message names the knob: {body}"
    );
}

/// A part with no filename becomes a string, so a large one is a mistake
/// rather than a value — and it is refused as such, not spilled.
#[tokio::test]
async fn a_value_part_is_capped_small() {
    let limits = Limits {
        file: 1 << 20,
        spill_at: 4,
        value: 8,
    };
    let req = upload("receive", &[("note", None, "far more than eight bytes")]);
    let (parts, body) = req.into_parts();

    let res = RpcCall::from_multipart_within(
        "receive".to_string(),
        Request::from_parts(parts, body),
        limits,
    )
    .await
    .expect_err("over the limit");

    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(body_of(res).await.contains("without a filename"));
}

// ------------------------------------------------------- streaming uploads

/// The other half of the wire: the file is left on it, and the handler is
/// what reads it.
#[tokio::test]
async fn a_streamed_file_is_handed_over_unread() {
    let big = "x".repeat(100_000);
    let req = upload(
        "store",
        &[("note", None, "keep it"), ("upload", Some("big.bin"), &big)],
    );

    let mut call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a streaming call");

    // The value in front of it was read on the way past.
    assert_eq!(call.arg::<String>("note").ok(), Some("keep it".to_string()));

    let mut stream = call.upload("upload").expect("the file is there");
    assert_eq!(stream.name(), "big.bin");
    assert_eq!(stream.content_type(), Some("text/plain"));
    assert_eq!(
        stream.read_so_far(),
        0,
        "not a byte of it has been taken yet"
    );

    let mut total = 0;
    while let Some(chunk) = stream.chunk().await.expect("a chunk") {
        total += chunk.len();
    }
    assert_eq!(total, 100_000);
    assert_eq!(stream.read_so_far(), 100_000);
}

/// Straight to disk, which is what the type is for.
#[tokio::test]
async fn a_streamed_file_saves_as_it_arrives() {
    let req = upload(
        "store",
        &[("upload", Some("notes.txt"), "written as it came")],
    );
    let mut call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a call");
    let stream = call.upload("upload").expect("the file");

    let mut to = std::env::temp_dir();
    to.push(format!("rahti-streamed-{}", std::process::id()));

    let written = stream.save(&to).await.expect("saved");
    assert_eq!(written, 18);
    assert_eq!(
        std::fs::read(&to).expect("it is there"),
        b"written as it came"
    );
    let _ = std::fs::remove_file(&to);
}

/// The handler drives the read, so the cap has to be enforced there too —
/// otherwise a streaming rpc has no limit of its own at all.
#[tokio::test]
async fn a_streamed_file_over_the_limit_stops_partway() {
    // Delivered in pieces, the way a socket delivers one — a body handed over
    // whole would be refused on its first and only chunk, which proves the
    // limit but not that it stops reading.
    // A megabyte, so it is far larger than anything the parser underneath
    // holds at once and has to come out in pieces.
    const SIZE: usize = 1024 * 1024;
    let body = form(&[("upload", Some("big.bin"), &"x".repeat(SIZE))]);
    let chunks: Vec<Result<Bytes, std::io::Error>> = body
        .into_bytes()
        .chunks(8192)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();

    let req = Request::builder()
        .method("POST")
        .uri("/uploader")
        .header("host", "127.0.0.1:3000")
        .header("x-pp-function", "store")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header("cookie", format!("pp_csrf_3000={TOKEN}"))
        .header("x-csrf-token", TOKEN)
        .body(Body::from_stream(tokio_stream::iter(chunks)))
        .expect("a request");

    let mut call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a call");
    let mut stream = call.upload("upload").expect("the file");

    // Below what `max_file` would allow, so this is the type's own check.
    stream.set_limit_for_test(1024);

    let mut hit = None;
    loop {
        match stream.chunk().await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(e) => {
                hit = Some(e);
                break;
            }
        }
    }

    let e = hit.expect("it stopped");
    assert_eq!(e.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        stream.read_so_far() < SIZE,
        "it refused the file on its way in, not after all of it had arrived"
    );
}

/// The hazard the whole design is arranged around: a payload with the file
/// first leaves everything behind it unreachable, and saying "you did not
/// send `note`" would send someone looking in the wrong place.
#[tokio::test]
async fn an_argument_sent_after_the_file_says_so() {
    let req = upload(
        "store",
        &[
            ("upload", Some("a.txt"), "first"),
            ("note", None, "too late"),
        ],
    );
    let call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a call");

    let res = call
        .arg::<String>("note")
        .expect_err("it was never reached");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_of(res).await;
    assert!(body.contains("before the `upload` file"), "{body}");
    assert!(body.contains("put the file last"), "{body}");
}

/// A streaming rpc called without a file at all.
#[tokio::test]
async fn a_streaming_call_with_no_file_is_reported_against_its_name() {
    let req = upload("store", &[("note", None, "no file here")]);
    let mut call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a call");

    let res = call.upload("upload").expect_err("nothing was uploaded");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(res).await.contains("without its `upload` file"));
}

/// A streaming rpc reached by a JSON call: the client only posts `FormData`
/// when there is a `File` in the payload, so this is a call that forgot one.
#[tokio::test]
async fn a_streaming_call_that_is_not_an_upload_is_refused() {
    let res = RpcCall::from_request_streaming(call("store", r#"{"note":"hi"}"#), "upload")
        .await
        .expect_err("not multipart");

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_of(res).await;
    assert!(body.contains("reads as it arrives"), "{body}");
    assert!(body.contains("input.files[0]"), "{body}");
}

/// The escape hatch, for a handler that turns out to want the bytes after
/// all: it spools them the ordinary way rather than being stuck.
#[tokio::test]
async fn a_streamed_file_can_still_be_collected() {
    let req = upload("store", &[("upload", Some("a.txt"), "collected after all")]);
    let mut call = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect("a call");
    let stream = call.upload("upload").expect("the file");

    let file = stream.collect(4).await.expect("collected");
    assert!(file.is_spilled(), "past the threshold it was given");
    assert_eq!(
        file.text().await.expect("readable").as_deref(),
        Some("collected after all")
    );
}

/// CSRF is checked before a byte of the body is read, on this path as much as
/// on the other one.
#[tokio::test]
async fn a_streaming_call_without_the_token_is_refused() {
    let req = Request::builder()
        .method("POST")
        .uri("/uploader")
        .header("host", "127.0.0.1:3000")
        .header("x-pp-function", "store")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .header("cookie", format!("pp_csrf_3000={TOKEN}"))
        .body(Body::from(form(&[("upload", Some("a.txt"), "x")])))
        .expect("a request");

    let res = RpcCall::from_request_streaming(req, "upload")
        .await
        .expect_err("no token");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------ stream

/// The pieces, read off the stream itself.
///
/// Deliberately not through [`rpc_stream`]: that response ends when the
/// process is told to stop, and the shutdown channel is process-wide, so a
/// response read here would be racing whatever `shutdown::tests` is doing on
/// another thread. What [`rpc_stream`] adds — the wire, the marker — is
/// asserted on its own below.
async fn chunks_of(stream: RpcStream) -> Vec<String> {
    stream.chunks.collect().await
}

/// `text/event-stream`, and marked as an rpc answer so no layer replaces it.
#[tokio::test]
async fn a_stream_answers_on_the_wire_the_client_reads() {
    let res = rpc_stream(RpcStream::from_iter([1, 2, 3]));

    assert_eq!(res.status(), StatusCode::OK);
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "{content_type}"
    );
    assert!(
        res.extensions().get::<RpcBody>().is_some(),
        "an rpc answer is marked"
    );
}

/// One `data:` line per piece, which is the line the client parses.
///
/// Reading the body to its end also proves the response *has* one: the client
/// waits for the stream to close before `onStreamComplete`, so a stream whose
/// pieces have run out has to end rather than sit open.
#[tokio::test]
async fn a_finished_stream_closes_its_response() {
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        body_of(rpc_stream(RpcStream::from_iter([1]))),
    )
    .await
    .expect("the response ends when the pieces run out");

    assert!(body.contains("data: 1\n"), "{body}");
}

/// Each piece is one JSON value on one line, because that is what the client
/// parses out of a `data:` line.
#[tokio::test]
async fn every_piece_is_one_json_value() {
    let chunks = chunks_of(RpcStream::from_iter([
        serde_json::json!({ "at": 1, "label": "one" }),
        serde_json::json!({ "at": 2, "label": "two" }),
    ]))
    .await;

    assert_eq!(
        chunks,
        vec![r#"{"at":1,"label":"one"}"#, r#"{"at":2,"label":"two"}"#]
    );
    assert!(chunks.iter().all(|c| !c.contains('\n')), "{chunks:?}");
}

#[tokio::test]
async fn a_channel_carries_what_is_sent_through_it() {
    let (send, stream) = RpcStream::channel(4);

    tokio::spawn(async move {
        for at in 1..=3 {
            assert!(send.send(serde_json::json!({ "at": at })).await);
        }
    });

    assert_eq!(
        chunks_of(stream).await,
        vec![r#"{"at":1}"#, r#"{"at":2}"#, r#"{"at":3}"#]
    );
}

/// `false` from `send` is the signal to stop working, not an error to report:
/// the browser navigated away, or the call was aborted.
#[tokio::test]
async fn sending_into_a_stream_nobody_reads_says_so() {
    let (send, stream) = RpcStream::channel(1);
    drop(stream);

    assert!(!send.send(1).await);
    assert!(!send.is_open());
}

/// There is no way to say "this piece failed" on a wire the client reads as a
/// sequence of values, so the stream ends instead of sending half of one.
///
/// A map with keys JSON cannot name is the ordinary way to hit this. A
/// non-finite float is *not*: serde writes those as `null` and reports no
/// error, so the stream carries a `null` piece and continues.
#[tokio::test]
async fn a_piece_that_cannot_be_written_ends_the_stream() {
    let ok = std::collections::HashMap::from([("at", 1)]);
    let broken = std::collections::HashMap::from([((1, 2), 3)]);

    assert_eq!(
        chunks_of(RpcStream::from_iter([ok])).await,
        vec![r#"{"at":1}"#]
    );
    assert!(chunks_of(RpcStream::from_iter([broken])).await.is_empty());
}

#[tokio::test]
async fn a_stream_with_nothing_in_it_is_an_empty_answer() {
    assert!(
        chunks_of(RpcStream::from_iter(Vec::<u8>::new()))
            .await
            .is_empty()
    );
    assert_eq!(
        rpc_stream(RpcStream::from_iter(Vec::<u8>::new())).status(),
        StatusCode::OK
    );
}
