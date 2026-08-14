//! The `#[socket]` attribute.
//!
//! A function the browser can hold a conversation with. Like `#[rpc]`, the
//! attribute leaves the function exactly as written and adds a shim beside
//! it that speaks the client's wire:
//!
//! ```ignore
//! #[socket]
//! pub async fn chat(room: String, mut socket: Socket) -> rahti::Result<()> { … }
//! ```
//!
//! becomes that function, plus
//!
//! ```ignore
//! pub async fn __rahti_socket_chat(__args: Map<String, Value>, __socket: Socket) {
//!     let room = match rahti::ws::arg::<String>(&__args, "chat", "room") { … };
//!     let __sender = __socket.sender();
//!     if let Err(error) = chat(room, __socket).await {
//!         rahti::ws::failed(&__sender, error).await;
//!     }
//! }
//! ```
//!
//! The arguments arrive as the connection's first frame — one JSON object,
//! the payload `pp.socket("chat", { room })` sends on open — and are read by
//! the parameter's own name, exactly as an rpc's are.
//!
//! Unlike an rpc, a socket has no response to write, so dispatch is entirely
//! the link-time registry: every `#[socket]` submits a `SocketEntry`, and the
//! one endpoint `rahti::ws::routes()` serves looks the name up there. That
//! makes socket names application-wide wherever they are written — a page, a
//! component, a library — and `rahti-build` refuses two files in this project
//! claiming one, while the boot-time check catches what a library linked in.
//!
//! The shim's names differ from an rpc shim's in one deliberate way: there is
//! nothing to return. A refused argument or an `Err` from the handler becomes
//! an `{"error": …}` frame followed by a close, sent by the runtime helpers,
//! because inside an open connection a frame is the only wire a failure has.

use proc_macro2::{Delimiter, Ident, TokenStream, TokenTree};
use quote::quote;

use crate::parse::Error;
use crate::rpc::{first_generic, head_ident, span_of, split_top_level};

#[cfg(test)]
#[path = "tests/socket.rs"]
mod tests;

/// One parameter the first frame has to supply.
struct Param {
    name: Ident,
    ty: TokenStream,
}

pub fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream, Error> {
    let guarded = parse_attr(attr)?;

    let toks: Vec<TokenTree> = item.into_iter().collect();
    let keyword = toks
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "fn"))
        .ok_or_else(|| Error {
            span: span_of(&toks),
            msg: "rahti: `#[socket]` goes on a function.\n  \
                  A socket is an `async fn` whose last parameter is the \
                  `rahti::ws::Socket` it talks through."
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

    let is_async = toks[..keyword]
        .iter()
        .any(|tt| matches!(tt, TokenTree::Ident(id) if id == "async"));
    if !is_async {
        // A socket that cannot await cannot receive: `recv` is the whole
        // conversation, and it only exists behind `.await`.
        return Err(Error {
            span: name.span(),
            msg: format!(
                "rahti: `{name}` is not `async`, and a socket has to be.\n  \
                 The conversation is `socket.recv().await` and \
                 `socket.send(…).await` — a synchronous function could never \
                 hold it."
            ),
        });
    }

    let params_at = keyword + 2;
    let (params, _socket) = match toks.get(params_at) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            parse_params(g.stream(), &name)?
        }
        // A generic parameter list sits exactly here, as in `#[rpc]`.
        _ => {
            return Err(Error {
                span: name.span(),
                msg: format!(
                    "rahti: `{name}` cannot be a generic `#[socket]`.\n  \
                     The browser opens one wire, so the function it reaches \
                     has to have one signature."
                ),
            });
        }
    };

    let fallible = return_shape(&toks[params_at + 1..], &name)?;

    let shim = Ident::new(&format!("__rahti_socket_{name}"), name.span());
    let wire = name.to_string();

    let reads = params.iter().map(|param| {
        let ident = &param.name;
        let ty = &param.ty;
        let key = ident.to_string();
        quote! {
            let #ident = match ::rahti::ws::arg::<#ty>(&__args, #wire, #key) {
                ::core::result::Result::Ok(value) => value,
                ::core::result::Result::Err(message) => {
                    return ::rahti::ws::refuse(__socket, message).await;
                }
            };
        }
    });

    let names = params.iter().map(|param| &param.name);
    let call = quote! { #name(#(#names,)* __socket).await };
    // Only the fallible shape keeps a sender: the handler consumes the
    // socket, and the error frame has to travel on something.
    let answer = if fallible {
        quote! {
            let __sender = __socket.sender();
            if let ::core::result::Result::Err(error) = #call {
                ::rahti::ws::failed(&__sender, error).await;
            }
        }
    } else {
        quote! { #call; }
    };

    let original: TokenStream = toks.into_iter().collect();
    let doc = format!("Wire shim for `{name}`, written by `#[socket]`.");
    let auth = guarded;

    let registration = quote! {
        ::rahti::inventory::submit! {
            ::rahti::ws::SocketEntry {
                name: #wire,
                source: ::core::concat!(::core::file!(), ":", ::core::line!()),
                auth: #auth,
                handler: |__args, __socket| ::std::boxed::Box::pin(#shim(__args, __socket)),
            }
        }
    };

    Ok(quote! {
        #original

        #[doc = #doc]
        #[doc(hidden)]
        pub async fn #shim(
            __args: ::rahti::serde_json::Map<::std::string::String, ::rahti::serde_json::Value>,
            __socket: ::rahti::ws::Socket,
        ) {
            #(#reads)*
            #answer
        }

        #registration
    })
}

/// `#[socket]`, or `#[socket(auth)]`. The same vocabulary as `#[rpc]`, for
/// the same reason: whether a session is required is the wire's business;
/// who counts as an administrator is the application's.
fn parse_attr(attr: TokenStream) -> Result<bool, Error> {
    let toks: Vec<TokenTree> = attr.into_iter().collect();

    match toks.as_slice() {
        [] => Ok(false),
        [TokenTree::Ident(id)] if id == "auth" => Ok(true),
        _ => Err(Error {
            span: span_of(&toks),
            msg: "rahti: `#[socket]` takes `auth`, or nothing.\n  \
                  `#[socket]` is a connection anyone may open; \
                  `#[socket(auth)]` refuses the handshake with a 401 unless \
                  the browser is carrying a session.\n  \
                  The name the browser connects with is the function's own \
                  name either way."
                .into(),
        }),
    }
}

/// Read the parameter list: the payload parameters, and the one `Socket`.
///
/// The socket has to be last for the same reason an `RpcUpload` does — not
/// because the compiler cares, but because the signature is what an author
/// reads: everything before the socket arrives in the first frame, and the
/// socket is where the conversation starts.
fn parse_params(stream: TokenStream, func: &Ident) -> Result<(Vec<Param>, Ident), Error> {
    let mut params = Vec::new();
    let mut socket: Option<Ident> = None;

    for group in split_top_level(stream) {
        let mut toks = group.into_iter();

        let name = match toks.next() {
            Some(TokenTree::Ident(id)) if id == "self" => {
                return Err(Error {
                    span: id.span(),
                    msg: format!(
                        "rahti: `{func}` takes `self`, so it belongs to a type \
                         and cannot be a `#[socket]`.\n  \
                         A socket is opened by name from the browser, with \
                         nothing to open it on."
                    ),
                });
            }
            // `mut socket: Socket` — the usual spelling, since `recv` needs it.
            Some(TokenTree::Ident(id)) if id == "mut" => match toks.next() {
                Some(TokenTree::Ident(id)) => id,
                _ => {
                    return Err(Error {
                        span: id.span(),
                        msg: format!("rahti: expected a name after `mut` in `{func}`."),
                    });
                }
            },
            Some(TokenTree::Ident(id)) => id,
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

        let ty_toks: Vec<TokenTree> = ty.clone().into_iter().collect();
        if head_ident(&ty_toks).as_deref() == Some("Socket") {
            if let Some(first) = &socket {
                return Err(Error {
                    span: name.span(),
                    msg: format!(
                        "rahti: `{func}` takes two `Socket` parameters, \
                         `{first}` and `{name}`, and a connection is one wire.\n  \
                         Take one `Socket`; clone `socket.sender()` inside the \
                         function when another task needs to send."
                    ),
                });
            }
            socket = Some(name);
            continue;
        }

        if socket.is_some() {
            return Err(Error {
                span: name.span(),
                msg: format!(
                    "rahti: `{name}` in `{func}` is written after the `Socket`, \
                     and the socket has to be last.\n  \
                     Everything before it arrives in the connection's first \
                     frame; the socket is where the conversation starts, so \
                     the signature reads in the order things happen."
                ),
            });
        }

        check_owned(&ty, &name, func)?;
        params.push(Param { name, ty });
    }

    let Some(socket) = socket else {
        return Err(Error {
            span: func.span(),
            msg: format!(
                "rahti: `{func}` takes no `Socket`, so it has nothing to talk \
                 through.\n  \
                 Add it as the last parameter: \
                 `async fn {func}(…, mut socket: Socket)` — \
                 `use rahti::ws::Socket;`."
            ),
        });
    };

    Ok((params, socket))
}

/// Arguments are deserialized out of the first frame, so they have to own
/// their data — `#[rpc]`'s rule, with the socket's wording.
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
            "rahti: `{name}` in `{func}` is borrowed, and a socket argument is \
             built from the first frame rather than pointed at it.\n  \
             Take it by value — `String` rather than `&str`, `Vec<T>` rather \
             than `&[T]`."
        ),
    })
}

/// What the function may return: nothing, `()`, or `Result<()>`.
///
/// Returns whether it is fallible. Anything else is refused, because there is
/// no response to carry a value back on — a socket answers through its
/// `Socket`, frame by frame, and a returned value would be dropped silently.
fn return_shape(rest: &[TokenTree], name: &Ident) -> Result<bool, Error> {
    let arrow = rest.windows(2).position(|pair| {
        matches!(&pair[0], TokenTree::Punct(p) if p.as_char() == '-')
            && matches!(&pair[1], TokenTree::Punct(p) if p.as_char() == '>')
    });

    let Some(arrow) = arrow else {
        return Ok(false);
    };

    let ty: Vec<TokenTree> = rest[arrow + 2..]
        .iter()
        .take_while(|tt| {
            !matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                && !matches!(tt, TokenTree::Ident(id) if id == "where")
        })
        .cloned()
        .collect();

    if is_unit(&ty) {
        return Ok(false);
    }

    if head_ident(&ty).as_deref() == Some("Result") {
        let value = first_generic(&ty);
        if value.is_empty() || is_unit(&value) {
            return Ok(true);
        }
    }

    Err(Error {
        span: name.span(),
        msg: format!(
            "rahti: `{name}` returns a value, and a socket has no response to \
             carry one on.\n  \
             It answers through its `Socket`, frame by frame — return nothing, \
             or `rahti::Result<()>` so an `Err` reaches the browser as an \
             error frame."
        ),
    })
}

/// Whether a return type is `()` — an empty parenthesis group.
fn is_unit(ty: &[TokenTree]) -> bool {
    matches!(
        ty,
        [TokenTree::Group(g)]
            if g.delimiter() == Delimiter::Parenthesis && g.stream().is_empty()
    )
}
