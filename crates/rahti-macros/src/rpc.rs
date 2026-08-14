//! The `#[rpc]` attribute.
//!
//! A function the browser can call. The attribute leaves the function exactly
//! as written — it is still an ordinary Rust function the page can call
//! itself — and adds a shim beside it that speaks the client's wire:
//!
//! ```ignore
//! #[rpc]
//! pub async fn greet(name: String) -> Greeting { … }
//! ```
//!
//! becomes that function, plus
//!
//! ```ignore
//! pub async fn __rahti_rpc_greet(__call: RpcCall) -> RpcResponse {
//!     let name = match __call.arg::<String>("name") { … };
//!     rahti::rpc_ok(&greet(name).await)
//! }
//! ```
//!
//! Arguments come out of the JSON payload by the parameter's own name, so
//! `pp.rpc("greet", { name: "Ada" })` is the call — the names on both sides
//! are the same word, which is the point of naming them at all.
//!
//! A function returning `Result` is handled by looking at the tokens rather
//! than at a trait: `rahti::Error` cannot implement `Serialize` without
//! becoming part of the wire, and a blanket impl over `Serialize` would
//! overlap with one over `Result` anyway. The return type is read here, where
//! the answer is already written down.
//!
//! `rahti-build` finds these by scanning `page.rs` and `src/components/` for
//! the attribute and writes a dispatcher that calls the right shim, so a
//! page's rpcs cost a `match` the compiler checked.
//!
//! That scan can only see this project. A component library is compiled
//! separately and its source is never read here, so `#[rpc]` also submits an
//! entry to the link-time registry in `rahti::registry`, which the linker
//! fills from every crate in the binary. A name a page does not own is looked
//! up there. The shim is the same either way — only what reaches it differs,
//! since a page's rpcs answer on its own URL and a component's answer on
//! whichever page rendered it.

use proc_macro2::{Delimiter, Ident, Span, TokenStream, TokenTree};
use quote::quote;

use crate::parse::Error;

#[cfg(test)]
#[path = "tests/rpc.rs"]
mod tests;

/// One parameter, as the payload has to supply it.
struct Param {
    name: Ident,
    ty: TokenStream,
    source: Source,
}

/// Where a parameter's value comes from.
///
/// An upload is not JSON — `RpcClient` switches to `FormData` the moment the
/// payload holds a `File` — so a file parameter is read from the parts rather
/// than deserialized. Which of the two a parameter is, is its type: `RpcFile`
/// cannot be `DeserializeOwned` without pretending an upload is a value.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// Deserialized out of the payload by name.
    Payload,
    /// One uploaded file, required.
    File,
    /// One uploaded file, or none.
    OptionalFile,
    /// Every file sent under this name — a `FileList`.
    Files,
    /// One file, read by the handler as it arrives rather than taken off the
    /// wire first. Changes how the whole call is read, so it carries rules the
    /// others do not: see [`check_upload`].
    Upload,
}

/// What the function hands back.
#[derive(Clone, Copy)]
struct Shape {
    /// `-> Result<T>`: an `Err` becomes the status and message the client
    /// throws.
    fallible: bool,
    /// `-> RpcStream`: the answer arrives in pieces, as `text/event-stream`.
    streaming: bool,
}

pub fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream, Error> {
    let guarded = parse_attr(attr)?;

    let toks: Vec<TokenTree> = item.into_iter().collect();
    let keyword = toks
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "fn"))
        .ok_or_else(|| Error {
            span: span_of(&toks),
            msg: "rahti: `#[rpc]` goes on a function.\n  \
                  An rpc is a `fn` that takes its arguments and returns \
                  something serializable."
                .into(),
        })?;

    let name = match toks.get(keyword + 1) {
        Some(TokenTree::Ident(name)) => name.clone(),
        _ => {
            return Err(Error {
                span: toks[keyword].span(),
                msg: "rahti: expected a name after `fn`.".into(),
            });
        }
    };

    let params_at = keyword + 2;
    let params = match toks.get(params_at) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            parse_params(g.stream(), g.span(), &name)?
        }
        // A generic parameter list sits exactly here, and a generic rpc has no
        // single function for the dispatcher to call.
        _ => {
            return Err(Error {
                span: name.span(),
                msg: format!(
                    "rahti: `{name}` cannot be a generic `#[rpc]`.\n  \
                     The browser sends one payload, so the function it reaches \
                     has to have one signature."
                ),
            });
        }
    };

    let is_async = toks[..keyword]
        .iter()
        .any(|tt| matches!(tt, TokenTree::Ident(id) if id == "async"));
    let shape = return_shape(&toks[params_at + 1..], &name)?;
    let streamed = check_upload(&params, &name)?;

    let shim = Ident::new(&format!("__rahti_rpc_{name}"), name.span());
    let reads = params.iter().map(|param| {
        let ident = &param.name;
        let ty = &param.ty;
        let key = ident.to_string();
        // Files come out of the parts, everything else out of the payload.
        let read = match param.source {
            Source::Payload => quote! { __call.arg::<#ty>(#key) },
            Source::File => quote! { __call.file(#key) },
            Source::OptionalFile => quote! { __call.optional_file(#key) },
            Source::Files => quote! { __call.files(#key) },
            Source::Upload => quote! { __call.upload(#key) },
        };
        quote! {
            let #ident = match #read {
                ::core::result::Result::Ok(value) => value,
                ::core::result::Result::Err(response) => return response,
            };
        }
    });
    let names = params.iter().map(|param| &param.name);
    let call = if is_async {
        quote! { #name(#(#names),*).await }
    } else {
        quote! { #name(#(#names),*) }
    };
    // One `Ok` value, written the way its type has to travel.
    let send = |value: TokenStream| {
        if shape.streaming {
            quote! { ::rahti::rpc_stream(#value) }
        } else {
            quote! { ::rahti::rpc_ok(&(#value)) }
        }
    };
    let answer = if shape.fallible {
        let sent = send(quote! { value });
        quote! {
            match #call {
                ::core::result::Result::Ok(value) => #sent,
                ::core::result::Result::Err(error) => ::rahti::rpc_failed(error),
            }
        }
    } else {
        send(call)
    };

    let original: TokenStream = toks.into_iter().collect();
    let doc = format!("Wire shim for `{name}`, written by `#[rpc]`.");
    // Taking a file out of the call mutates it; asking for a value does not,
    // and a needless `mut` is a warning against generated code.
    let binding = if params.iter().any(|p| p.source != Source::Payload) {
        quote! { mut __call }
    } else {
        quote! { __call }
    };

    // Before the body is read, and before the function is called: a call with
    // no session is refused without the server parsing what it sent.
    let check_auth = if guarded {
        quote! {
            if let ::core::result::Result::Err(response) = ::rahti::auth::require() {
                return response;
            }
        }
    } else {
        quote! {}
    };

    // How much of the body to read is the shim's to decide, which is why the
    // dispatcher hands it the request rather than a finished call: an rpc
    // that streams its file has to stop at the file instead of taking it.
    let read_call = match &streamed {
        Some(field) => quote! {
            ::rahti::RpcCall::from_request_streaming(__req, #field).await
        },
        None => quote! { ::rahti::RpcCall::from_request(__req).await },
    };

    // The registry entry, so an rpc written in a *library* is reachable at
    // all. A page's rpcs are dispatched by a `match` the build writes, which
    // needs to have seen the file; a dependency's source is never scanned, so
    // without this a component library could ship a button whose server half
    // could never be called. See `registry.rs` in the runtime.
    //
    // Submitted for every `#[rpc]`, not only a library's: the macro cannot
    // tell where it is, and the codegen excludes the app's own page rpcs from
    // the fallback by name.
    let wire = name.to_string();
    let registration = quote! {
        ::rahti::inventory::submit! {
            ::rahti::RpcEntry {
                name: #wire,
                source: ::core::concat!(::core::file!(), ":", ::core::line!()),
                handler: |__req| ::std::boxed::Box::pin(#shim(__req)),
            }
        }
    };

    Ok(quote! {
        #original

        #[doc = #doc]
        #[doc(hidden)]
        pub async fn #shim(__req: ::rahti::axum::extract::Request) -> ::rahti::RpcResponse {
            #check_auth
            let #binding = match #read_call {
                ::core::result::Result::Ok(call) => call,
                ::core::result::Result::Err(response) => return response,
            };
            #(#reads)*
            #answer
        }

        #registration
    })
}

/// What the attribute itself says.
///
/// `#[rpc]` is an open call, and `#[rpc(auth)]` is one that answers only while
/// the browser carries a session. That is the whole vocabulary, deliberately:
/// there is no role here to name, because who counts as an administrator is a
/// question about the application's users rather than about the wire.
///
/// Returns whether the guard is wanted.
fn parse_attr(attr: TokenStream) -> Result<bool, Error> {
    let toks: Vec<TokenTree> = attr.into_iter().collect();

    match toks.as_slice() {
        [] => Ok(false),
        [TokenTree::Ident(id)] if id == "auth" => Ok(true),
        _ => Err(Error {
            span: span_of(&toks),
            msg: "rahti: `#[rpc]` takes `auth`, or nothing.\n  \
                  `#[rpc]` is a call anyone may make; `#[rpc(auth)]` is one \
                  that answers 401 unless the browser is carrying a session.\n  \
                  The name the browser calls is the function's own name either \
                  way."
                .into(),
        }),
    }
}

/// Read `name: Type, other: Type` into something the shim can ask for.
fn parse_params(stream: TokenStream, span: Span, func: &Ident) -> Result<Vec<Param>, Error> {
    let mut params = Vec::new();

    for group in split_top_level(stream) {
        let mut toks = group.into_iter();

        let name = match toks.next() {
            Some(TokenTree::Ident(id)) if id == "self" => {
                return Err(Error {
                    span: id.span(),
                    msg: format!(
                        "rahti: `{func}` takes `self`, so it belongs to a type and \
                         cannot be an `#[rpc]`.\n  \
                         An rpc is called by name from the browser, with nothing \
                         to call it on."
                    ),
                });
            }
            Some(TokenTree::Ident(id)) => id,
            // A pattern — `(a, b): (i32, i32)`, `_: Config`. The payload names
            // its arguments, so the parameter has to have a name to match.
            Some(tt) => {
                return Err(Error {
                    span: tt.span(),
                    msg: format!(
                        "rahti: every argument of `{func}` needs a plain name.\n  \
                         The browser sends `{{ name: value }}`, and the name it \
                         sends is the parameter's."
                    ),
                });
            }
            None => continue,
        };

        match toks.next() {
            Some(TokenTree::Punct(p)) if p.as_char() == ':' => {}
            _ => {
                return Err(Error {
                    span: name.span(),
                    msg: format!("rahti: `{name}` in `{func}` has no type."),
                });
            }
        }

        let ty: TokenStream = toks.collect();
        if ty.is_empty() {
            return Err(Error {
                span: name.span(),
                msg: format!("rahti: `{name}` in `{func}` has no type."),
            });
        }
        let source = source_of(&ty);
        if source == Source::Payload {
            check_owned(&ty, &name, func)?;
        }

        params.push(Param { name, ty, source });
    }

    // Not an error — a call with no arguments is ordinary. Only here so the
    // span is used and the reason is on record.
    let _ = span;
    Ok(params)
}

/// Arguments are deserialized out of the payload, so they have to own their
/// data. Said here rather than left to a `DeserializeOwned` bound, whose error
/// points at generated code.
fn check_owned(ty: &TokenStream, name: &Ident, func: &Ident) -> Result<(), Error> {
    let borrowed = matches!(
        ty.clone().into_iter().next(),
        Some(TokenTree::Punct(ref p)) if p.as_char() == '&'
    );
    if !borrowed {
        return Ok(());
    }

    Err(Error {
        span: name.span(),
        msg: format!(
            "rahti: `{name}` in `{func}` is borrowed, and an rpc argument is built \
             from the payload rather than pointed at it.\n  \
             Take it by value — `String` rather than `&str`, `Vec<T>` rather than \
             `&[T]`."
        ),
    })
}

/// What the function returns, read off the return type.
///
/// `Result<T>`, `rahti::Result<T>` and `std::result::Result<T, E>` all count
/// as fallible: the name that matters is the last one before the generics.
/// `RpcStream` — on its own, or inside a `Result` — is the streaming form.
fn return_shape(rest: &[TokenTree], name: &Ident) -> Result<Shape, Error> {
    let arrow = rest.windows(2).position(|pair| {
        matches!(&pair[0], TokenTree::Punct(p) if p.as_char() == '-')
            && matches!(&pair[1], TokenTree::Punct(p) if p.as_char() == '>')
    });

    let Some(arrow) = arrow else {
        return Err(Error {
            span: name.span(),
            msg: format!(
                "rahti: `{name}` returns nothing, so there is nothing to send back.\n  \
                 Give it a return type — the value is serialized to JSON, \
                 `-> rahti::Result<T>` is the fallible form, and `-> RpcStream` \
                 sends the answer in pieces."
            ),
        });
    };

    // Everything between the arrow and the body, or the `where` clause.
    let ty: Vec<TokenTree> = rest[arrow + 2..]
        .iter()
        .take_while(|tt| {
            !matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                && !matches!(tt, TokenTree::Ident(id) if id == "where")
        })
        .cloned()
        .collect();

    let fallible = head_ident(&ty).as_deref() == Some("Result");
    let value = if fallible { first_generic(&ty) } else { ty };

    Ok(Shape {
        fallible,
        streaming: head_ident(&value).as_deref() == Some("RpcStream"),
    })
}

/// The rules an `RpcUpload` parameter carries, checked here so that breaking
/// one is a compile error rather than a call that fails in the browser.
///
/// Multipart is sequential: a part cannot be read before the ones in front of
/// it, and the handler reading the `RpcUpload` *is* the thing that consumes
/// it. So everything the function wants beside the file has to have been read
/// on the way to it, which gives two rules:
///
/// - **At most one.** Two streams would each have to be read before the
///   other.
/// - **Last.** Not because the compiler needs the order, but because the
///   signature is what an author reads: a parameter written after the stream
///   would look like it is read after the stream, and nothing is.
///
/// Returns the field name to stream, when there is one.
fn check_upload(params: &[Param], func: &Ident) -> Result<Option<String>, Error> {
    let mut uploads = params.iter().filter(|p| p.source == Source::Upload);

    let Some(upload) = uploads.next() else {
        return Ok(None);
    };

    if let Some(second) = uploads.next() {
        return Err(Error {
            span: second.name.span(),
            msg: format!(
                "rahti: `{func}` takes more than one `RpcUpload`, and it can \
                 take at most one.\n  \
                 An `RpcUpload` is the request body itself, read as it \
                 arrives — two of them would each have to be read before the \
                 other.\n  \
                 Take `RpcFile` instead for the ones that can be spooled."
            ),
        });
    }

    let last = params.last().map(|p| &p.name);
    if last.map(|last| last.to_string()) != Some(upload.name.to_string()) {
        return Err(Error {
            span: upload.name.span(),
            msg: format!(
                "rahti: `{func}` takes `{}: RpcUpload` before its other \
                 parameters, and an `RpcUpload` has to be the last one.\n  \
                 It is read as it arrives, so everything the browser sends \
                 after it is unreachable — putting it last is what makes the \
                 signature say so.\n  \
                 The payload has to match: \
                 `pp.rpc(\"{func}\", {{ …, {}: input.files[0] }})`.",
                upload.name, upload.name
            ),
        });
    }

    Ok(Some(upload.name.to_string()))
}

/// Which of the wires a parameter is read from.
fn source_of(ty: &TokenStream) -> Source {
    let toks: Vec<TokenTree> = ty.clone().into_iter().collect();
    let head = head_ident(&toks);

    match head.as_deref() {
        Some("RpcUpload") => Source::Upload,
        Some("RpcFile") => Source::File,
        Some("Option") if head_ident(&first_generic(&toks)).as_deref() == Some("RpcFile") => {
            Source::OptionalFile
        }
        Some("Vec") if head_ident(&first_generic(&toks)).as_deref() == Some("RpcFile") => {
            Source::Files
        }
        _ => Source::Payload,
    }
}

/// The name a type is written under, generics aside: `rahti::Result<T>` is
/// `Result`, `Vec<RpcFile>` is `Vec`.
pub(crate) fn head_ident(ty: &[TokenTree]) -> Option<String> {
    ty.iter()
        .take_while(|tt| !matches!(tt, TokenTree::Punct(p) if p.as_char() == '<'))
        .filter_map(|tt| match tt {
            TokenTree::Ident(id) => Some(id.to_string()),
            _ => None,
        })
        .last()
}

/// The first type inside `<…>`: the `T` of `Option<T>`, `Vec<T>`, `Result<T>`.
pub(crate) fn first_generic(ty: &[TokenTree]) -> Vec<TokenTree> {
    let mut depth = 0i32;
    let mut inner: Vec<TokenTree> = Vec::new();

    for tt in ty {
        match tt {
            TokenTree::Punct(p) if p.as_char() == '<' => {
                depth += 1;
                if depth == 1 {
                    continue; // The bracket itself is not part of the type.
                }
            }
            TokenTree::Punct(p) if p.as_char() == '>' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            // `Result<T, E>` — only the value side matters here.
            TokenTree::Punct(p) if p.as_char() == ',' && depth == 1 => break,
            _ => {}
        }
        if depth >= 1 {
            inner.push(tt.clone());
        }
    }
    inner
}

/// Split on commas that are not inside brackets, so `HashMap<String, i32>`
/// stays one type.
pub(crate) fn split_top_level(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut out: Vec<Vec<TokenTree>> = Vec::new();
    let mut current: Vec<TokenTree> = Vec::new();
    // Angle brackets are punctuation rather than a group, so they are the one
    // nesting the token stream does not track on its own.
    let mut depth = 0i32;

    for tt in stream {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
            TokenTree::Punct(p) if p.as_char() == '>' => depth -= 1,
            TokenTree::Punct(p) if p.as_char() == ',' && depth <= 0 => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                continue;
            }
            _ => {}
        }
        current.push(tt);
    }

    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub(crate) fn span_of(toks: &[TokenTree]) -> Span {
    toks.first().map_or_else(Span::call_site, |tt| tt.span())
}
