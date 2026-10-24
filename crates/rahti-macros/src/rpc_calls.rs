//! Checking a template's `pp.rpc` calls against the `#[rpc]`s they name.
//!
//! `rahti-js` reads each call out of the client code; this module writes, for
//! each one, constants the compiler evaluates:
//!
//! ```ignore
//! const __RAHTI_RPC_CALL_0: &::rahti::rpc_check::Signature = &__RAHTI_RPC_greet;
//! const _: () = ::rahti::rpc_check::key(__RAHTI_RPC_CALL_0, "name");
//! const _: () = ::rahti::rpc_check::complete(__RAHTI_RPC_CALL_0, &["name"]);
//! ```
//!
//! The first names the signature `#[rpc]` wrote beside the function, so it
//! resolves through the module the template is written in: a name no `#[rpc]`
//! there declares is rustc's own "cannot find value" error, suggestions
//! included. The rest are evaluated by `rahti::rpc_check`, whose panics are
//! compile errors. Each is spanned on the token it is about — the name, the
//! key, the payload — so the error lands there rather than on `html!`.

use std::ops::Range;

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned};
use rahti_js::{Code, Payload, RpcName};

use crate::parse::{ClientKind, ClientSource, Error};

/// The checks for every `pp.rpc` call in `source`. `next` numbers the calls
/// across one block, so each gets its own constant.
pub fn checks(source: &ClientSource, next: &mut usize) -> Result<TokenStream, Error> {
    let code = match source.kind {
        ClientKind::Expression => Code::Expression,
        ClientKind::Handler => Code::Handler,
        ClientKind::Script => Code::Script,
    };
    let calls = rahti_js::rpc_calls(code, &source.source);
    if calls.is_empty() {
        return Ok(TokenStream::new());
    }
    let locator = Locator::new(source);

    let mut out = TokenStream::new();
    for call in calls {
        let (name, name_span) = match &call.name {
            RpcName::Literal { value, span } => (value, locator.span(span)),
            RpcName::NotLiteral { span } => {
                return Err(Error {
                    span: locator.span(span),
                    msg: crate::authoring::rpc_name_not_literal(&source.source[span.clone()]),
                });
            }
            RpcName::Missing { span } => {
                return Err(Error {
                    span: locator.span(span),
                    msg: crate::authoring::rpc_name_missing(),
                });
            }
        };
        // Not an identifier, so not an rpc: `#[rpc]` is on a Rust function.
        // Spelled as one, it would be a name-resolution error naming
        // something nobody wrote; the server answers it with a 404 either way.
        if !is_rust_identifier(name) {
            return Err(Error {
                span: name_span,
                msg: format!(
                    "rahti: `pp.rpc(\"{name}\", …)` names no `#[rpc]`: an rpc's name is its \
                     Rust function's name.\n  Write the function's name, as in \
                     `pp.rpc(\"save_contact\", {{ … }})`."
                ),
            });
        }

        let signature = format_ident!("__RAHTI_RPC_CALL_{}", *next);
        *next += 1;
        // Located on the name, so rustc's "cannot find value" lands there and
        // its note points at the `#[rpc]` spelled most like it.
        let marker = crate::rpc::marker_ident(name, Span::call_site().located_at(name_span));
        let line = locator.line(&call.name);
        out.extend(quote! {
            #[allow(non_upper_case_globals)]
            const #signature: &::rahti::rpc_check::Signature = &#marker;
        });

        match &call.payload {
            Payload::Missing => out.extend(quote_spanned! {name_span=>
                const _: () = ::rahti::rpc_check::no_payload(#signature, #line);
            }),
            Payload::Unchecked => {}
            Payload::Object { keys, open, span } => {
                for key in keys {
                    let at = locator.span(&key.span);
                    let name = &key.name;
                    out.extend(quote_spanned! {at=>
                        const _: () = ::rahti::rpc_check::key(#signature, #name, #line);
                    });
                    if key.value != rahti_js::Literal::Unknown && !open {
                        let at = locator.span(&key.value_span);
                        let kind = proc_macro2::Ident::new(&format!("{:?}", key.value), at);
                        out.extend(quote_spanned! {at=>
                            const _: () = ::rahti::rpc_check::value(
                                #signature, #name, ::rahti::rpc_check::Literal::#kind, #line
                            );
                        });
                    }
                }
                // With a spread or a computed key, more may be sent than is
                // written: the keys written are checked, the rest are not.
                if !open {
                    let at = locator.span(span);
                    let names = keys.iter().map(|key| &key.name);
                    out.extend(quote_spanned! {at=>
                        const _: () =
                            ::rahti::rpc_check::complete(#signature, &[#(#names),*], #line);
                    });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests/rpc_calls.rs"]
mod tests;

fn is_rust_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Maps a byte range in a binding's source back to the token that wrote it.
///
/// A binding's JavaScript is also Rust tokens, in the same order, so each
/// token is found in the source by its text, one after the other. A script
/// body is a single literal, and a span cannot point inside a literal on
/// stable Rust, so everything in a script lands on the script's first
/// character, and the message says which line of the file it is about.
struct Locator {
    whole: Span,
    /// The line the source starts on, for code written as one literal.
    literal_line: Option<usize>,
    source: String,
    /// Byte range in the source, and the span of the token or group there.
    tokens: Vec<(Range<usize>, Span)>,
}

impl Locator {
    fn new(source: &ClientSource) -> Self {
        let mut tokens = Vec::new();
        if let Some(stream) = &source.tokens {
            let mut cursor = 0;
            walk(stream.clone(), &source.source, &mut cursor, &mut tokens);
        }
        let literal = source.tokens.is_none();
        // Outside a macro — in a unit test — there is no source to point
        // into, and asking for one panics.
        let in_macro = proc_macro::is_available();
        Locator {
            whole: if literal && in_macro {
                Span::call_site().located_at(source.span.unwrap().start().into())
            } else {
                source.span
            },
            literal_line: (literal && in_macro).then(|| source.span.start().line),
            source: source.source.clone(),
            tokens,
        }
    }

    /// `" (line 12)"` for a call in code written as one literal, where the
    /// span alone does not say; empty where it does.
    fn line(&self, name: &RpcName) -> String {
        let (RpcName::Literal { span, .. }
        | RpcName::NotLiteral { span }
        | RpcName::Missing { span }) = name;
        match self.literal_line {
            Some(first) => {
                let before = self.source.get(..span.start).unwrap_or_default();
                format!(" (line {})", first + before.matches('\n').count())
            }
            None => String::new(),
        }
    }

    fn span(&self, range: &Range<usize>) -> Span {
        self.tokens
            .iter()
            .find(|(at, _)| at == range)
            .or_else(|| self.tokens.iter().find(|(at, _)| at.contains(&range.start)))
            .map_or(self.whole, |(_, span)| *span)
    }
}

fn walk(
    stream: TokenStream,
    source: &str,
    cursor: &mut usize,
    out: &mut Vec<(Range<usize>, Span)>,
) {
    for token in stream {
        match token {
            TokenTree::Group(group) => {
                let (open, close) = match group.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                let start = find(source, cursor, open);
                walk(group.stream(), source, cursor, out);
                let end = find(source, cursor, close);
                if let (Some(start), Some(end)) = (start, end)
                    && !open.is_empty()
                {
                    out.push((start.start..end.end, group.span()));
                }
            }
            token => {
                let text = token
                    .span()
                    .source_text()
                    .unwrap_or_else(|| token.to_string())
                    .replace("\r\n", "\n");
                if let Some(at) = find(source, cursor, &text) {
                    out.push((at, token.span()));
                }
            }
        }
    }
}

/// The next occurrence of `text` at or after `cursor`, which moves past it.
fn find(source: &str, cursor: &mut usize, text: &str) -> Option<Range<usize>> {
    if text.is_empty() {
        return None;
    }
    let start = *cursor + source.get(*cursor..)?.find(text)?;
    *cursor = start + text.len();
    Some(start..*cursor)
}
