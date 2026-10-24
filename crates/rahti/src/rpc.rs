//! Calling a Rust function from the browser.
//!
//! The client runtime already knows how to do this. `pp.rpc("greet", { name })`
//! POSTs to the URL the page is already on, names the function in a header,
//! and reads JSON back:
//!
//! ```text
//! POST /greeter
//! X-PP-Function: greet
//! X-CSRF-Token: 6f1c…
//! Content-Type: application/json
//!
//! {"name":"Ada"}
//! ```
//!
//! So the server side is a POST on the page's own URL. `#[rpc]` on a function
//! in a `page.rs` is what puts it there: the attribute writes a shim that
//! reads the arguments out of that payload and serializes what comes back,
//! and `rahti-build` writes the dispatcher that picks the shim by name.
//! Nothing is registered at runtime — by the time the server boots it is a
//! `match` on a `&str`.
//!
//! ## Components
//!
//! A component is markup and a script that render wherever they are called, so
//! `pp.rpc` inside one posts to whatever page that turned out to be. `#[rpc]`
//! in a `src/components/` file is therefore not given a URL: `rahti-build`
//! collects those into one dispatcher and has every page fall through to it
//! after its own names. A button carries its server half in its own file, and
//! the page that uses it says nothing about either.
//!
//! The cost is that a component's names are app-wide rather than file-local,
//! which the build refuses to let go quiet: two components claiming one name,
//! or a page and a component claiming one, is a build error.
//!
//! Two markers keep the response on the wire the client expects. [`RpcBody`]
//! tells the layout and error layers to leave it alone, the way [`PageBody`]
//! tells them to act; and every failure comes back as `{"error": "…"}`,
//! because that is the field the client reads before it throws.
//!
//! ## CSRF
//!
//! The call is a cookie-authenticated POST from the page's own origin, so it
//! needs the double-submit check the client is already written for: [`csrf`]
//! puts a random token in a readable `pp_csrf` cookie on the way out of a
//! page, the client copies it into `X-CSRF-Token`, and [`RpcCall`] refuses
//! anything where the two do not agree. A form posted from another site
//! carries the cookie but cannot read it, so it cannot produce the header.
//!
//! The layer is only wired up when the app has an `#[rpc]` somewhere, so an
//! app that never calls one pays nothing and sets no cookie.

use std::any::Any;
use std::collections::HashMap;
use std::pin::Pin;

use axum::body::{Bytes, to_bytes};
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};

use super::error::Error;
use super::html::PageBody;
use super::upload::{
    Limits, MAX_FILE_ENV, MAX_UPLOAD_ENV, RpcFile, RpcUpload, Spool, SpoolError, max_upload,
};

#[cfg(test)]
#[path = "tests/rpc.rs"]
mod tests;

/// Names the function the client wants. Sent by `RpcClient`.
const FUNCTION_HEADER: &str = "x-pp-function";
/// The client's half of the double-submit pair.
const CSRF_HEADER: &str = "x-csrf-token";
/// The cookie's half. The client looks for `pp_csrf_<port>` first and falls
/// back to this, so two apps on one host do not overwrite each other's token.
const CSRF_COOKIE: &str = "pp_csrf";

/// Arguments arrive as one JSON object built in the browser. Generous for
/// that, and small enough that a misdirected upload is refused rather than
/// buffered — an upload comes the other way, as multipart.
const MAX_BODY: usize = 1024 * 1024;

// How large an upload may be lives in `upload.rs` with the rest of the
// limits: `max_upload` for the body as a whole, which the generated router
// hangs on the rpc routes as an `axum::extract::DefaultBodyLimit` so an
// oversized body is refused as it arrives, and `max_file` for any one part,
// which is enforced here as the part is spooled.

/// Marks a response as an RPC answer.
///
/// Layout and error layers sit above every route in their subtree, including
/// this one. Without the marker, JSON returned from a page's URL would come
/// back wrapped in `<!DOCTYPE html>`, and a failure would be replaced by the
/// error boundary's page — neither of which the client can parse.
#[derive(Debug, Clone, Copy)]
pub struct RpcBody;

/// What an `#[rpc]` shim returns. An ordinary response; named so generated
/// code reads as what it is.
pub type RpcResponse = Response;

/// One call, parsed and checked.
///
/// Built by the shim `#[rpc]` wrote, which is the only thing that reads
/// arguments out of it. The dispatcher hands the shim the request rather than
/// the call, because how much of the body to read is the shim's to decide: an
/// rpc that takes an [`RpcUpload`] stops at the file instead of taking it.
#[derive(Debug)]
pub struct RpcCall {
    name: String,
    args: Map<String, Value>,
    /// Uploaded parts, by the field they arrived under. A `FileList` sends
    /// several under one name, which is why this is a list.
    files: HashMap<String, Vec<RpcFile>>,
    /// The file the handler will read itself, when this is a streaming call.
    upload: Option<RpcUpload>,
    /// The field the parse stopped at, when it stopped early. What makes the
    /// difference between "you did not send `note`" and "you sent it after
    /// the file, where it cannot be read".
    stopped_at: Option<String>,
}

impl RpcCall {
    /// The function the client named.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Read one argument by the parameter's own name.
    ///
    /// A missing key reads as JSON `null`, so an `Option<T>` parameter is
    /// optional and everything else is required — the same rule serde
    /// applies to a struct field.
    ///
    /// The error is already a response: the shim returns it as-is, because
    /// there is nothing useful the generated code could add. Which is also
    /// why it is not boxed — clippy would rather it were, but unwrapping a
    /// box is a line generated code would have to carry for every argument.
    #[allow(clippy::result_large_err)]
    pub fn arg<T: DeserializeOwned>(&self, name: &str) -> Result<T, Response> {
        let value = self.args.get(name).cloned().unwrap_or(Value::Null);

        if let Ok(value) = serde_json::from_value::<T>(value.clone()) {
            return Ok(value);
        }

        // A form part has no types: `FormData` turns every value into a
        // string on the way out, so `{ count: 2 }` arrives as `"2"` and an
        // object arrives as the text of its JSON. Read that back only when
        // the value as it stands does not fit — a `String` parameter still
        // gets the characters the browser sent, and a `serde_json::Value`
        // parameter gets the string, since a string already is one.
        if let Value::String(raw) = &value
            && let Ok(value) = serde_json::from_str::<T>(raw)
        {
            return Ok(value);
        }

        // A streaming call stopped reading at the file, so an argument that
        // is simply absent here may well have been sent — behind it, where
        // there is no way to reach it. Saying "you did not send `note`" for
        // that would send someone looking in the wrong place.
        if let Some(field) = &self.stopped_at
            && !self.args.contains_key(name)
        {
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "rahti: `{}` did not receive its `{name}` argument before \
                     the `{field}` file.\n  \
                     `{field}` is read as it arrives, so everything sent after \
                     it is unreachable — put the file last in the payload: \
                     `pp.rpc(\"{}\", {{ {name}: …, {field}: input.files[0] }})`.",
                    self.name, self.name
                ),
            ));
        }

        let missing = !self.args.contains_key(name);
        let code = if missing {
            "RAHTI_RPC_MISSING_ARGUMENT"
        } else {
            "RAHTI_RPC_ARGUMENT_TYPE"
        };
        let expected = std::any::type_name::<T>();
        let received = if missing {
            "missing"
        } else {
            match &value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            }
        };
        let message = format!(
            "rahti[{code}]: `{}` could not read its `{name}` argument. Expected {expected}; received {received}.\n  Send a value matching the Rust parameter in pp.rpc(\"{}\", {{ {name}: … }}).",
            self.name, self.name
        );
        super::dev::record_server("warn", "rpc-contract", &message);
        Err(json_error_with_details(
            StatusCode::BAD_REQUEST,
            message,
            Some(&serde_json::json!({
                "code": code, "rpc": self.name, "argument": name,
                "expectedRustType": expected, "receivedType": received,
                "hint": "Send a value matching the Rust parameter; inspect the RPC contract or source declaration.",
            })),
        ))
    }

    /// One uploaded file, by the field it arrived under.
    ///
    /// Taken out of the call rather than copied: an upload is bytes, and the
    /// shim asks for each one exactly once.
    #[allow(clippy::result_large_err)]
    pub fn file(&mut self, name: &str) -> Result<RpcFile, Response> {
        match self.optional_file(name)? {
            Some(file) => Ok(file),
            None => Err(json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "rahti: `{}` was called without its `{name}` file.\n  \
                     Send it as `pp.rpc(\"{}\", {{ {name}: input.files[0] }})` — a \
                     `File` in the payload is what makes the call an upload.",
                    self.name, self.name
                ),
            )),
        }
    }

    /// The same, for a parameter the caller may leave out.
    #[allow(clippy::result_large_err)]
    pub fn optional_file(&mut self, name: &str) -> Result<Option<RpcFile>, Response> {
        let files = self.files.get_mut(name);
        Ok(files.and_then(|files| (!files.is_empty()).then(|| files.remove(0))))
    }

    /// Every file sent under one name, which is what a `FileList` becomes.
    /// An empty list is an ordinary answer: nothing was picked.
    #[allow(clippy::result_large_err)]
    pub fn files(&mut self, name: &str) -> Result<Vec<RpcFile>, Response> {
        Ok(self.files.remove(name).unwrap_or_default())
    }

    /// The whole payload, for a function that would rather read it itself.
    pub fn args(&self) -> &Map<String, Value> {
        &self.args
    }

    /// The file this call is streaming, by the field it arrived under.
    ///
    /// Only a call built by [`RpcCall::from_request_streaming`] has one, which
    /// is the calls whose rpc takes an [`RpcUpload`].
    #[allow(clippy::result_large_err)]
    pub fn upload(&mut self, name: &str) -> Result<RpcUpload, Response> {
        match self.upload.take() {
            Some(upload) => Ok(upload),
            None => Err(json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "rahti: `{}` was called without its `{name}` file.\n  \
                     It takes an `RpcUpload`, so the payload has to carry a \
                     `File` under `{name}` — and it has to be the last thing \
                     in the payload, because the arguments beside it are read \
                     on the way to it.",
                    self.name
                ),
            )),
        }
    }

    /// Take the request apart: check who is calling, then what they sent.
    ///
    /// The `Err` side is the finished response to send back, so a caller does
    /// nothing but return it.
    pub async fn from_request(req: Request) -> Result<Self, Response> {
        let (parts, body) = req.into_parts();
        let name = rpc_name(&parts.headers)?;

        let content_type = header_str(&parts.headers, header::CONTENT_TYPE.as_str()).unwrap_or("");
        if content_type.starts_with("multipart/form-data") {
            // The browser sets the boundary itself, so the client never spells
            // this content type — a `File` anywhere in the payload is what
            // switches `RpcClient` from JSON to `FormData`.
            return Self::from_multipart(name, Request::from_parts(parts, body)).await;
        }

        let bytes = to_bytes(body, MAX_BODY).await.map_err(|_| {
            json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("rahti: the payload for `{name}` is larger than {MAX_BODY} bytes."),
            )
        })?;

        // An empty body is a call with no arguments — `pp.rpc("reload")`
        // still sends `{}`, but a hand-written client need not.
        let args = if bytes.is_empty() {
            Map::new()
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(map)) => map,
                Ok(_) | Err(_) => {
                    return Err(json_error(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "rahti: the payload for `{name}` is not a JSON object.\n  \
                             Arguments are named, so they arrive as `{{ \"count\": 1 }}`."
                        ),
                    ));
                }
            }
        };

        Ok(RpcCall {
            name,
            args,
            files: HashMap::new(),
            upload: None,
            stopped_at: None,
        })
    }

    /// The upload wire: one part per value, files among them.
    ///
    /// `RpcClient.createFormData` writes every non-file value as a string —
    /// `String(value)`, or its JSON when it is an object — so the parts that
    /// are not files land in `args` as strings and [`RpcCall::arg`] reads them
    /// back. A part with a filename is a file, which is the rule the browser
    /// itself uses.
    /// Read only as far as the file, and leave the file itself on the wire.
    ///
    /// What an rpc taking an [`RpcUpload`] is dispatched through. The parts in
    /// front of `field` are read as arguments the ordinary way; the part named
    /// `field` becomes the [`RpcUpload`] the handler drives; the parts behind
    /// it are never reached, because reading them would mean reading past a
    /// file nobody has consumed yet.
    ///
    /// A payload with the file first is therefore a payload whose arguments
    /// are unreachable. That is not silent: the call remembers where it
    /// stopped, and [`RpcCall::arg`] says so instead of reporting a missing
    /// value.
    pub async fn from_request_streaming(req: Request, field: &str) -> Result<Self, Response> {
        let (parts, body) = req.into_parts();
        let name = rpc_name(&parts.headers)?;

        let content_type = header_str(&parts.headers, header::CONTENT_TYPE.as_str()).unwrap_or("");
        let boundary = multer::parse_boundary(content_type).map_err(|_| {
            json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "rahti: `{name}` takes a file it reads as it arrives, so the \
                     call has to be an upload.\n  \
                     Send it as `pp.rpc(\"{name}\", {{ …, {field}: input.files[0] }})` \
                     — a `File` in the payload is what makes the client post \
                     `FormData` instead of JSON."
                ),
            )
        })?;

        // multer rather than `axum::extract::Multipart`: axum's hands out a
        // `Field` borrowed from the parser, and a borrowed field cannot be
        // given to a handler. multer's is owned, which is the whole reason
        // this path exists.
        let mut parser = Box::new(multer::Multipart::new(body.into_data_stream(), boundary));

        let mut args = Map::new();
        let limits = Limits::from_env(MAX_BODY);

        loop {
            let mut part = match parser.next_field().await {
                Ok(Some(part)) => part,
                Ok(None) => break,
                Err(e) => {
                    return Err(json_error(
                        StatusCode::BAD_REQUEST,
                        format!("rahti: the upload to `{name}` stopped early — {e}."),
                    ));
                }
            };

            let Some(part_name) = part.name().map(str::to_string) else {
                continue; // A part with no name belongs to no parameter.
            };

            // The one the handler asked for: stop here and hand it over with
            // the body still open behind it.
            if part_name == field && part.file_name().is_some() {
                return Ok(RpcCall {
                    name,
                    args,
                    files: HashMap::new(),
                    upload: Some(RpcUpload::new(part, parser, limits.file)),
                    stopped_at: Some(field.to_string()),
                });
            }

            // Anything else in front of it is an argument. A file that is not
            // the one being streamed is read the ordinary way — but held, not
            // spilled, because a streaming rpc has no `RpcFile` parameter to
            // hand it to and it is only here to be skipped past.
            let mut spool = Spool::new(limits.value);
            loop {
                match part.chunk().await {
                    Ok(Some(chunk)) => {
                        if let Err(e) = spool.push(chunk, usize::MAX).await {
                            return Err(spool_error(&name, &part_name, false, e));
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return Err(json_error(
                            StatusCode::BAD_REQUEST,
                            format!("rahti: `{part_name}` did not arrive whole — {e}."),
                        ));
                    }
                }
            }

            if part.file_name().is_none() {
                let bytes = spool.into_memory();
                args.insert(
                    part_name,
                    Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                );
            }
        }

        // Every part read, and none of them was the file.
        Ok(RpcCall {
            name,
            args,
            files: HashMap::new(),
            upload: None,
            stopped_at: None,
        })
    }

    async fn from_multipart(name: String, req: Request) -> Result<Self, Response> {
        Self::from_multipart_within(name, req, Limits::from_env(MAX_BODY)).await
    }

    /// The same, against limits the caller chose — which is how the spill is
    /// tested without a 256 KiB body.
    async fn from_multipart_within(
        name: String,
        req: Request,
        limits: Limits,
    ) -> Result<Self, Response> {
        let mut multipart = Multipart::from_request(req, &()).await.map_err(|e| {
            json_error(
                StatusCode::BAD_REQUEST,
                format!("rahti: the upload to `{name}` is not readable — {e}."),
            )
        })?;

        let mut args = Map::new();
        let mut files: HashMap<String, Vec<RpcFile>> = HashMap::new();

        loop {
            let mut field = match multipart.next_field().await {
                Ok(Some(field)) => field,
                Ok(None) => break,
                // Over the router's limit, or a truncated body. Either way the
                // parts already read are not the call the caller meant to make.
                Err(e) => {
                    return Err(json_error(
                        e.status(),
                        format!(
                            "rahti: the upload to `{name}` stopped early — {e}.\n  \
                             Uploads are limited to {} MiB.",
                            max_upload() / (1024 * 1024)
                        ),
                    ));
                }
            };

            let Some(field_name) = field.name().map(str::to_string) else {
                continue; // A part with no name belongs to no parameter.
            };
            let file_name = field.file_name().map(str::to_string);
            let content_type = field.content_type().map(str::to_string);

            // A part that is a file may be as large as the per-file limit and
            // goes to disk past the threshold. A part that is a value becomes
            // a string, so it is held and capped at what an argument payload
            // is allowed to be — an upload arriving under a name with no
            // filename is a mistake, not a value.
            let is_file = file_name.is_some();
            let (limit, spill_at) = limits.for_part(is_file);

            // The part is read chunk by chunk rather than with `field.bytes()`:
            // what makes an upload cost the threshold instead of its size.
            let mut spool = Spool::new(limit);
            loop {
                match field.chunk().await {
                    Ok(Some(chunk)) => {
                        if let Err(e) = spool.push(chunk, spill_at).await {
                            return Err(spool_error(&name, &field_name, is_file, e));
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return Err(json_error(
                            e.status(),
                            format!("rahti: `{field_name}` did not arrive whole — {e}."),
                        ));
                    }
                }
            }

            match file_name {
                Some(file_name) => {
                    let body = spool
                        .finish()
                        .await
                        .map_err(|e| spool_error(&name, &field_name, true, e))?;
                    files.entry(field_name).or_default().push(RpcFile {
                        name: file_name,
                        content_type,
                        body,
                    });
                }
                None => {
                    let bytes = spool.into_memory();
                    args.insert(
                        field_name,
                        Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                    );
                }
            }
        }

        Ok(RpcCall {
            name,
            args,
            files,
            upload: None,
            stopped_at: None,
        })
    }
}

/// Who is calling, and whether they are allowed to.
///
/// The name is a header, so it is known before a byte of the body is read —
/// which is what lets the dispatcher pick a shim and leave reading the body
/// to it.
#[allow(clippy::result_large_err)]
pub fn rpc_name(headers: &HeaderMap) -> Result<String, Response> {
    let name = match header_str(headers, FUNCTION_HEADER) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => {
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                "rahti: this POST named no function.\n  \
                 Call it as `pp.rpc(\"name\", { … })` — the client runtime \
                 sends the name in `X-PP-Function`.",
            ));
        }
    };

    check_csrf(headers)?;
    Ok(name)
}

/// What went wrong spooling one part, as the response to send back.
///
/// A part over its limit is the caller's mistake and says which limit it hit;
/// a failure to write one is the server's, and does not leak the path it was
/// writing to.
fn spool_error(name: &str, field: &str, is_file: bool, e: SpoolError) -> Response {
    match e {
        SpoolError::TooLarge(limit) if is_file => json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "rahti: the file sent to `{name}` as `{field}` is larger than \
                 {} MiB.\n  One file may be as large as {}, and the upload as a \
                 whole as large as {}.",
                limit / (1024 * 1024),
                MAX_FILE_ENV,
                MAX_UPLOAD_ENV,
            ),
        ),
        SpoolError::TooLarge(limit) => json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "rahti: `{field}` is larger than {limit} bytes.\n  \
                 It arrived without a filename, so it is an argument rather \
                 than a file — send a file as `pp.rpc(\"{name}\", {{ {field}: \
                 input.files[0] }})`."
            ),
        ),
        SpoolError::Io(e) => {
            super::dev::record_server(
                "error",
                "rpc-upload",
                &format!("rahti: could not spool `{field}` of the upload to `{name}`: {e}"),
            );
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("rahti: `{field}` could not be written while it arrived."),
            )
        }
    }
}

/// An answer that arrives in pieces.
///
/// Return one from an `#[rpc]` and the call becomes a stream: the response is
/// `text/event-stream`, each piece goes out as its own `data:` line, and the
/// browser reads them as they arrive instead of waiting for the last one.
///
/// ```ignore
/// #[rpc]
/// pub async fn tail(lines: usize) -> RpcStream {
///     let (send, stream) = RpcStream::channel(16);
///     tokio::spawn(async move {
///         for i in 0..lines {
///             if !send.send(&Line { number: i }).await {
///                 break; // The reader is gone.
///             }
///         }
///     });
///     stream
/// }
/// ```
///
/// The client side is the third argument to `pp.rpc`:
///
/// ```js
/// await pp.rpc("tail", { lines: 20 }, { onStream: line => …, onStreamComplete: () => … });
/// ```
///
/// Each piece is one JSON value on one line, which is what the client parses.
/// A piece that cannot be serialized ends the stream rather than sending half
/// of it — there is no way to say "this chunk failed" on a wire the client
/// reads as a sequence of values.
pub struct RpcStream {
    chunks: Pin<Box<dyn Stream<Item = String> + Send>>,
}

impl RpcStream {
    /// Stream whatever a `Stream` yields.
    pub fn new<S, T>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Serialize,
    {
        RpcStream {
            chunks: Box::pin(stream.map_while(|value| match serde_json::to_string(&value) {
                Ok(json) => Some(json),
                Err(e) => {
                    super::dev::record_server(
                        "error",
                        "rpc-stream",
                        &format!(
                            "rahti: an rpc stream ended — a chunk cannot be written as JSON: {e}"
                        ),
                    );
                    None
                }
            })),
        }
    }

    /// Stream a sequence that is already in hand.
    ///
    /// The pieces still arrive one at a time, so a long list renders as it
    /// comes rather than at the end.
    ///
    /// Not `FromIterator`: that trait has no way to say the items are
    /// serialized, and the name reads the same at the call site.
    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, T>(values: I) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: Send + 'static,
        T: Serialize,
    {
        RpcStream::new(tokio_stream::iter(values))
    }

    /// A stream fed by hand, for work that produces pieces as it goes.
    ///
    /// `capacity` is how far the producer may run ahead of the browser;
    /// beyond it [`RpcSender::send`] waits, which is the backpressure that
    /// keeps a fast producer from buffering the whole answer in memory.
    pub fn channel(capacity: usize) -> (RpcSender, RpcStream) {
        let (tx, rx) = mpsc::channel::<String>(capacity.max(1));
        (
            RpcSender(tx),
            RpcStream {
                chunks: Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)),
            },
        )
    }
}

impl std::fmt::Debug for RpcStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RpcStream")
    }
}

/// The writing end of [`RpcStream::channel`].
#[derive(Debug, Clone)]
pub struct RpcSender(mpsc::Sender<String>);

impl RpcSender {
    /// Send one piece. `false` means nobody is reading any more — the browser
    /// navigated away, or the call was aborted — and is the signal to stop
    /// producing rather than an error to report.
    pub async fn send<T: Serialize>(&self, value: T) -> bool {
        let json = match serde_json::to_string(&value) {
            Ok(json) => json,
            Err(e) => {
                super::dev::record_server(
                    "error",
                    "rpc-stream",
                    &format!("rahti: a chunk cannot be written as JSON — {e}"),
                );
                return false;
            }
        };
        self.0.send(json).await.is_ok()
    }

    /// Whether the browser is still reading, without sending anything.
    pub fn is_open(&self) -> bool {
        !self.0.is_closed()
    }
}

/// An [`RpcStream`] as the response the client reads.
///
/// Generated by `#[rpc]` for a function that returns one.
pub fn rpc_stream(stream: RpcStream) -> Response {
    let events = stream
        .chunks
        .map(|json| Ok::<Event, std::convert::Infallible>(Event::default().data(json)));

    // Two ways for the response to end, and it needs both.
    //
    // The pieces run out — that is the ordinary one, and it has to be said
    // explicitly: `merge` ends when *both* sides end, and the shutdown side
    // never does. Without the `chain`, a finished stream would leave the
    // response open and the browser waiting for a piece that is not coming.
    //
    // Or the process is told to stop. `axum::serve` shuts down by waiting for
    // every open connection, so a stream still running would hold Ctrl+C.
    let stop = tokio_stream::once(super::shutdown::watch()).then(|shutdown| async {
        shutdown.fired().await;
        None
    });
    let events = events.map(Some).chain(tokio_stream::once(None));
    let events = events.merge(stop).map_while(|event| event);

    // Keep-alive comments stop an idle proxy from deciding the connection
    // died. The client reads `data:` lines and ignores everything else.
    let mut res = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    res.extensions_mut().insert(RpcBody);
    res
}

/// A value on its way back to the browser.
///
/// Generated by `#[rpc]` around whatever the function returned.
pub fn rpc_ok<T: Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut res = body.into_response();
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            res.extensions_mut().insert(RpcBody);
            res
        }
        // Reported here rather than at compile time: `Serialize` is satisfied,
        // the value simply cannot be written — a map with non-string keys, a
        // float that is NaN.
        Err(e) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("rahti: the value an rpc returned cannot be written as JSON — {e}."),
        ),
    }
}

/// An `#[rpc]` that returned [`Err`].
///
/// The status is the one the function chose. Expected errors keep their public
/// message; unexpected production failures are redacted before JSON is built.
pub fn rpc_failed(error: Error) -> Response {
    error.record("rpc");
    let development = super::dev::enabled();
    json_error_with_details(
        error.status(),
        error.client_message(development),
        error.client_details(development),
    )
}

/// A name the dispatcher does not know.
pub fn rpc_unknown(name: &str) -> Response {
    json_error(
        StatusCode::NOT_FOUND,
        format!(
            "rahti: no rpc named `{name}` — not on this page, and not on any \
             component.\n  \
             Mark the function `#[rpc]` — the name the client sends is the \
             function's own."
        ),
    )
}

/// Turn a panic inside an rpc into JSON.
///
/// The counterpart of [`panic_response`](super::error::panic_response), which
/// answers with a page. Handed to `CatchPanicLayer::custom` by generated code.
pub fn rpc_panic_response(err: Box<dyn Any + Send + 'static>) -> Response {
    let message = if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked".to_string()
    };

    let error = Error::internal(message);
    error.record("rpc-panic");
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        error.client_message(super::dev::enabled()),
    )
}

/// `{"error": "…"}` — the base shape the client reads before it throws.
/// A validation failure also carries its `errors` field beside the summary.
///
/// Shared with `auth.rs`: a call refused for want of a session is refused on
/// the same wire as one refused for want of a token.
pub(crate) fn json_error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    json_error_with_details(status, message, None)
}

fn json_error_with_details(
    status: StatusCode,
    message: impl std::fmt::Display,
    details: Option<&Value>,
) -> Response {
    let mut body = Map::new();
    body.insert("error".to_string(), Value::String(message.to_string()));
    if let Some(Value::Object(fields)) = details {
        for (name, value) in fields {
            if name != "error" && name != "requestId" {
                body.insert(name.clone(), value.clone());
            }
        }
    } else if let Some(details) = details {
        body.insert("details".to_string(), details.clone());
    }
    if let Some(request_id) = super::observability::request_id() {
        body.insert(
            "requestId".to_string(),
            Value::String(request_id.to_string()),
        );
    }
    let mut res = (status, serde_json::to_vec(&body).unwrap_or_default()).into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res.extensions_mut().insert(RpcBody);
    res
}

// ------------------------------------------------------------------ CSRF

/// Hand every page a token the client can read.
///
/// Wired over the whole router by generated code, and only when the app has
/// an `#[rpc]` to protect. The cookie is deliberately not `HttpOnly`: the
/// client has to read it to send it back, and that is the whole mechanism —
/// what an attacker's page cannot do is read a cookie for another origin.
pub async fn csrf(req: Request, next: Next) -> Response {
    let name = cookie_name(req.headers());
    let existing = cookie(req.headers(), &name).map(str::to_string);

    let mut res = next.run(req).await;

    // Only on a page. A stylesheet or a JSON answer is not where a session
    // starts, and re-issuing on every request would race the calls in flight.
    if existing.is_some() || res.extensions().get::<PageBody>().is_none() {
        return res;
    }

    let Some(token) = new_token() else {
        return res;
    };

    // `SameSite=Lax` still sends the cookie on the top-level navigation that
    // renders the page, which is when the client needs to read it.
    let cookie = format!("{name}={token}; Path=/; SameSite=Lax");
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        res.headers_mut().append(header::SET_COOKIE, value);
    }
    res
}

/// Refuse a call whose header and cookie do not agree.
///
/// The refusal is the response to send, as in [`RpcCall::arg`].
#[allow(clippy::result_large_err)]
fn check_csrf(headers: &HeaderMap) -> Result<(), Response> {
    let name = cookie_name(headers);
    let sent = header_str(headers, CSRF_HEADER).unwrap_or("");
    let held = cookie(headers, &name).unwrap_or("");

    if !held.is_empty() && sent == held {
        return Ok(());
    }

    // 403 is what the client turns into "Permission denied", which is the
    // truthful thing to show: the call was refused, not the function.
    Err(json_error(
        StatusCode::FORBIDDEN,
        "rahti: this call carried no valid CSRF token.\n  \
         The token is set on the page it is called from — load the page first, \
         and send the `pp_csrf` cookie back in `X-CSRF-Token`.",
    ))
}

/// The cookie name for this request's origin.
///
/// Mirrors the client, which prefers `pp_csrf_<port>` whenever the page has
/// one in its URL — otherwise two apps on `localhost` would share a token.
fn cookie_name(headers: &HeaderMap) -> String {
    match header_str(headers, header::HOST.as_str())
        .and_then(|host| host.rsplit_once(':'))
        .map(|(_, port)| port)
        .filter(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
    {
        Some(port) => format!("{CSRF_COOKIE}_{port}"),
        None => CSRF_COOKIE.to_string(),
    }
}

/// 128 bits of hex, or `None` if the OS would not say.
///
/// A guessable token is not a CSRF defence, so this asks the operating system
/// rather than the clock. Failing is close to impossible and not worth
/// killing the server over: the call it protects is refused instead.
fn new_token() -> Option<String> {
    let mut bytes = [0u8; 16];
    if let Err(e) = getrandom::fill(&mut bytes) {
        super::dev::record_server(
            "error",
            "csrf",
            &format!("rahti: cannot read random bytes for a CSRF token — {e}"),
        );
        return None;
    }

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16)?);
        out.push(char::from_digit((byte & 0x0f) as u32, 16)?);
    }
    Some(out)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// One cookie out of the `Cookie` header, by name.
fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    header_str(headers, header::COOKIE.as_str())?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}
