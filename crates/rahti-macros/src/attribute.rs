//! The `#[component]` attribute.
//!
//! A component is an ordinary function that returns [`Html`]. The attribute
//! adds the two things a plain function cannot do for itself:
//!
//! - it names the block, stamping `pp-component="user_card_3b91f0ae"` on the
//!   element the function returns, so the client runtime can find it in the
//!   DOM the same way it finds a page or a layout;
//! - it holds the author to React's rule, which `html!` already enforces —
//!   one parent element out, props in.
//!
//! ```ignore
//! #[component]
//! pub fn UserCard(name: &str, children: Html) -> Html {
//!     html! {
//!         <article class="user-card">
//!             <h2>{name}</h2>
//!             <slot />
//!         </article>
//!     }
//! }
//! ```
//!
//! Naming happens by rewriting the markup rather than by wrapping the return
//! value: `html!` folds static markup into one `push_str`, so an attribute
//! written at compile time costs nothing at runtime, while patching the
//! string afterwards would mean re-scanning every response.

use proc_macro2::{Delimiter, Group, Spacing, Span, TokenStream, TokenTree};
use quote::quote;

use crate::component;
use crate::parse::Error;

#[cfg(test)]
#[path = "tests/attribute.rs"]
mod tests;

pub fn expand(attr: TokenStream, item: TokenStream, file: &str) -> Result<TokenStream, Error> {
    if let Some(tt) = attr.into_iter().next() {
        return Err(Error {
            span: tt.span(),
            msg: "rahti: `#[component]` takes no arguments.\n  \
                  The name comes from the function. To pin it yourself, write \
                  `pp-component=\"…\"` on the element the function returns."
                .into(),
        });
    }

    let toks: Vec<TokenTree> = item.into_iter().collect();
    let name = function_name(&toks)?;
    check_pascal_case(&name, &toks)?;
    check_returns_html(&toks, &name)?;

    let body_index = toks
        .iter()
        .rposition(|tt| matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace))
        .ok_or_else(|| Error {
            span: span_of(&toks),
            msg: format!("rahti: `{name}` has no body for `#[component]` to read."),
        })?;

    let TokenTree::Group(body) = &toks[body_index] else {
        unreachable!("selected by delimiter above");
    };

    let id = component::function_identifier(file, &name);
    let stamped = stamp_body(body.stream(), &id, &name, body.span())?;

    let mut rebuilt = Group::new(Delimiter::Brace, stamped);
    rebuilt.set_span(body.span());

    let signature: TokenStream = toks[..body_index].iter().cloned().collect();
    let companion = companion(&toks[..body_index], &name)?;
    let body = TokenTree::Group(rebuilt);

    // Components are PascalCase by convention, which is exactly what rustc
    // lints against. The convention is the point, so silence it here rather
    // than making every component file carry an `#[allow]`.
    Ok(quote! {
        #[allow(non_snake_case)]
        #signature
        #body
        #companion
    })
}

/// The named-props companion behind a component tag.
///
/// `html!` compiles `<Card title="Profile">…</Card>` into
///
/// ```ignore
/// Card { title: "Profile", children: … }.__rahti_call(Card)
/// ```
///
/// and this is the `Card` the literal builds: a braced struct sharing the
/// function's name — type namespace beside the function's value namespace,
/// so the one `use` that imports the component imports both, and an aliased
/// import (`use …::Card as Installed`) carries the pair along.
///
/// The struct is what turns written-order props into named ones: a struct
/// literal accepts its fields in any order, and its ordinary errors are the
/// diagnostics for an unknown or doubled prop. Only `__rahti_call`
/// knows the declaration order, and it is generated here, where that order
/// is visible; the call site never needs it. The struct is generic over
/// every field, so no parameter type — lifetimes included — is ever spelled
/// out; handing the fields to the function is what pins them.
///
/// It also carries a `Default`, which is what lets a tag leave a prop out.
/// `html!` closes every props literal with `..Default::default()`, so the
/// fields the author wrote are theirs and the rest come from here — a
/// component's defaults, reached the way React reaches them. The impl is
/// written out rather than derived so the bound is visible: every prop type
/// has to be `Default`, since the struct cannot know which fields a given
/// call site will omit. `&str`, `bool`, numbers, `String`, `Option<T>` and
/// `Html` all are; a prop that genuinely has no sensible default belongs in
/// an `Option`.
fn companion(signature: &[TokenTree], name: &str) -> Result<TokenStream, Error> {
    let fn_idx = signature
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "fn"))
        .expect("checked by `function_name`");
    let name_span = signature[fn_idx + 1].span();
    let ident = proc_macro2::Ident::new(name, name_span);

    let vis = visibility(&signature[..fn_idx]);
    let params = parameters(signature, fn_idx, name)?;
    let types: Vec<proc_macro2::Ident> = (0..params.len())
        .map(|i| quote::format_ident!("__RahtiP{i}"))
        .collect();

    Ok(quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, non_camel_case_types, dead_code)]
        #vis struct #ident<#(#types,)*> {
            #(pub #params: #types,)*
        }
        #[doc(hidden)]
        #[allow(non_snake_case, dead_code)]
        impl<#(#types: ::core::default::Default,)*> ::core::default::Default
            for #ident<#(#types,)*>
        {
            #[inline]
            fn default() -> Self {
                Self {
                    #(#params: ::core::default::Default::default(),)*
                }
            }
        }
        #[doc(hidden)]
        #[allow(non_snake_case, dead_code)]
        impl<#(#types,)*> #ident<#(#types,)*> {
            #[inline]
            #vis fn __rahti_call(
                self,
                __rahti_component: impl ::core::ops::FnOnce(#(#types),*) -> ::rahti::Html,
            ) -> ::rahti::Html {
                __rahti_component(#(self.#params),*)
            }
        }
    })
}

/// The function's visibility: the `pub` or `pub(…)` sitting right before
/// `fn`, and nothing when there is none. The companion items mirror it, so a
/// library component's shim crosses the crate boundary with the function.
fn visibility(before_fn: &[TokenTree]) -> TokenStream {
    let Some(i) = before_fn
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "pub"))
    else {
        return TokenStream::new();
    };
    let mut out = vec![before_fn[i].clone()];
    if let Some(TokenTree::Group(g)) = before_fn.get(i + 1) {
        // `pub(crate)`, `pub(super)` — the restriction travels along.
        if g.delimiter() == Delimiter::Parenthesis {
            out.push(before_fn[i + 1].clone());
        }
    }
    out.into_iter().collect()
}

/// The parameter names, in declaration order — the one thing the call-site
/// macro cannot know for itself.
fn parameters(
    signature: &[TokenTree],
    fn_idx: usize,
    name: &str,
) -> Result<Vec<proc_macro2::Ident>, Error> {
    // The parameter list is the first parenthesised group after the name
    // that sits outside the generic angle brackets. Angles arrive as bare
    // puncts, so they are counted by hand; the `>` of an `->` arrow inside a
    // bound (`impl Fn() -> Html`) closes nothing.
    let mut angle = 0i32;
    let mut group = None;
    for i in fn_idx + 2..signature.len() {
        match &signature[i] {
            TokenTree::Punct(p) if p.as_char() == '<' => angle += 1,
            TokenTree::Punct(p) if p.as_char() == '>' => {
                let arrow = matches!(&signature[i - 1], TokenTree::Punct(prev)
                    if prev.as_char() == '-' && prev.spacing() == Spacing::Joint);
                if !arrow {
                    angle -= 1;
                }
            }
            TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis && angle == 0 => {
                group = Some(g);
                break;
            }
            _ => {}
        }
    }
    let group = group.ok_or_else(|| Error {
        span: signature[fn_idx].span(),
        msg: format!("rahti: `{name}` has no parameter list."),
    })?;

    let toks: Vec<TokenTree> = group.stream().into_iter().collect();
    let mut pieces: Vec<Vec<TokenTree>> = Vec::new();
    let mut piece: Vec<TokenTree> = Vec::new();
    let mut angle = 0i32;
    for (i, tt) in toks.iter().enumerate() {
        match tt {
            TokenTree::Punct(p) if p.as_char() == '<' => angle += 1,
            TokenTree::Punct(p) if p.as_char() == '>' => {
                let arrow = i > 0
                    && matches!(&toks[i - 1], TokenTree::Punct(prev)
                        if prev.as_char() == '-' && prev.spacing() == Spacing::Joint);
                if !arrow {
                    angle -= 1;
                }
            }
            TokenTree::Punct(p) if p.as_char() == ',' && angle == 0 => {
                pieces.push(std::mem::take(&mut piece));
                continue;
            }
            _ => {}
        }
        piece.push(tt.clone());
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }

    let mut params = Vec::new();
    for piece in pieces {
        if piece
            .iter()
            .any(|tt| matches!(tt, TokenTree::Ident(id) if id == "self"))
        {
            return Err(Error {
                span: piece[0].span(),
                msg: format!(
                    "rahti: `{name}` takes `self`, and a component is a free function.\n  \
                     Props are plain arguments."
                ),
            });
        }
        // The pattern sits before the first `:`; its last identifier is the
        // name, which steps over a `mut`.
        let colon = piece
            .iter()
            .position(|tt| matches!(tt, TokenTree::Punct(p) if p.as_char() == ':'));
        let pattern = &piece[..colon.unwrap_or(piece.len())];
        let ident = pattern.iter().rev().find_map(|tt| match tt {
            TokenTree::Ident(id) if *id != "mut" => Some(id.clone()),
            _ => None,
        });
        match ident {
            Some(id) => params.push(id),
            None => {
                return Err(Error {
                    span: piece[0].span(),
                    msg: format!(
                        "rahti: `{name}` has a parameter with no name for a prop to \
                         reach.\n  A component takes its props as plain named arguments."
                    ),
                });
            }
        }
    }
    Ok(params)
}

/// Put the component's name into the markup it returns.
///
/// Only the tail expression is stamped — the value the function hands back.
/// An `html!` bound to a `let` on the way there is a fragment this component
/// assembles, not the component itself, and stamping it too would put the
/// same name on two elements for the client runtime to trip over.
///
/// Branches all count: `if cond { html! { … } } else { html! { … } }` returns
/// one element either way, so both are the component's root.
fn stamp_body(body: TokenStream, id: &str, name: &str, span: Span) -> Result<TokenStream, Error> {
    let toks: Vec<TokenTree> = body.into_iter().collect();
    let tail = toks
        .iter()
        .rposition(|tt| matches!(tt, TokenTree::Punct(p) if p.as_char() == ';'))
        .map_or(0, |i| i + 1);

    let head: TokenStream = toks[..tail].iter().cloned().collect();
    let mut found = 0usize;
    let stamped = stamp(
        toks[tail..].iter().cloned().collect(),
        id,
        Rule::Every,
        &mut found,
    );

    if found > 0 {
        let mut out = head;
        out.extend(stamped);
        return Ok(out);
    }

    // No markup in the tail: the function ends in `return html! { … };`, or
    // it does not build markup at all.
    let all: TokenStream = toks.iter().cloned().collect();
    let stamped = stamp(all, id, Rule::AfterReturn, &mut found);
    if found > 0 {
        return Ok(stamped);
    }

    Err(Error {
        span,
        msg: format!(
            "rahti: `{name}` is a `#[component]`, so it has to end with the markup it \
             renders.\n  Finish it with an `html!` block — the way a React component ends \
             in its `return`.\n  Building the markup into a `let` first hides it from the \
             attribute, which then has nothing to name."
        ),
    })
}

/// Which `html!` invocations to stamp.
#[derive(Clone, Copy)]
enum Rule {
    /// Every one in this run of tokens.
    Every,
    /// Only those written directly after `return`.
    AfterReturn,
}

/// Rewrite `html! { … }` into `html! { @component "id" … }`.
fn stamp(stream: TokenStream, id: &str, rule: Rule, found: &mut usize) -> TokenStream {
    let toks: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    let mut returning = false;

    while i < toks.len() {
        let is_html_call = matches!(&toks[i], TokenTree::Ident(ident) if ident == "html")
            && matches!(toks.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
            && matches!(toks.get(i + 2), Some(TokenTree::Group(_)));

        let take = is_html_call
            && match rule {
                Rule::Every => true,
                Rule::AfterReturn => returning,
            };

        if take {
            let TokenTree::Group(markup) = &toks[i + 2] else {
                unreachable!("checked above");
            };
            *found += 1;
            out.extend(toks[i..i + 2].iter().cloned());
            out.extend([TokenTree::Group(directive(markup, id))]);
            // Deliberately not recursing into the block we just named: an
            // `html!` interpolated inside it renders below this root, and the
            // root already carries the name.
            i += 3;
            returning = false;
            continue;
        }

        match &toks[i] {
            TokenTree::Group(g) => {
                let inner = stamp(g.stream(), id, rule, found);
                let mut rebuilt = Group::new(g.delimiter(), inner);
                rebuilt.set_span(g.span());
                out.extend([TokenTree::Group(rebuilt)]);
            }
            tt => out.extend([tt.clone()]),
        }

        returning = matches!(&toks[i], TokenTree::Ident(ident) if ident == "return");
        i += 1;
    }
    out
}

/// A copy of the markup group with `@component "id"` written in front.
fn directive(markup: &Group, id: &str) -> Group {
    let mut stream = quote! { @component #id };
    stream.extend(markup.stream());
    let mut group = Group::new(markup.delimiter(), stream);
    group.set_span(markup.span());
    group
}

/// The name of the function the attribute was written on.
fn function_name(toks: &[TokenTree]) -> Result<String, Error> {
    let keyword = toks
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "fn"))
        .ok_or_else(|| Error {
            span: span_of(toks),
            msg: "rahti: `#[component]` goes on a function.\n  \
                  A component is a `fn` that takes its props and returns `Html`."
                .into(),
        })?;

    match toks.get(keyword + 1) {
        Some(TokenTree::Ident(name)) => Ok(name.to_string()),
        _ => Err(Error {
            span: toks[keyword].span(),
            msg: "rahti: expected a name after `fn`.".into(),
        }),
    }
}

/// Components are PascalCase, as in React — and as everywhere else that
/// writes them, so that a call reads as markup rather than as a helper.
fn check_pascal_case(name: &str, toks: &[TokenTree]) -> Result<(), Error> {
    if name.starts_with(|c: char| c.is_uppercase()) {
        return Ok(());
    }
    let span = toks
        .iter()
        .find(|tt| matches!(tt, TokenTree::Ident(id) if id == name))
        .map_or_else(|| span_of(toks), |tt| tt.span());

    Err(Error {
        span,
        msg: format!(
            "rahti: a component is named in PascalCase, and `{name}` is not.\n  \
             Rename it to `{}` — the capital is what separates a component from an \
             ordinary function at the call site.",
            pascal_case(name)
        ),
    })
}

/// A component that returns anything else has nothing to stamp, and the
/// error rustc would give for it points into expanded code.
fn check_returns_html(toks: &[TokenTree], name: &str) -> Result<(), Error> {
    let arrow = toks.windows(2).any(|pair| {
        matches!(&pair[0], TokenTree::Punct(p) if p.as_char() == '-')
            && matches!(&pair[1], TokenTree::Punct(p) if p.as_char() == '>')
    });
    if arrow {
        return Ok(());
    }
    Err(Error {
        span: span_of(toks),
        msg: format!(
            "rahti: `{name}` returns nothing, so it cannot be a component.\n  \
             Give it `-> Html` and end it with the markup it renders."
        ),
    })
}

fn pascal_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut capitalize = true;
    for c in name.chars() {
        if c == '_' {
            capitalize = true;
            continue;
        }
        if capitalize {
            out.extend(c.to_uppercase());
            capitalize = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn span_of(toks: &[TokenTree]) -> Span {
    toks.first().map_or_else(Span::call_site, |tt| tt.span())
}

/// Report a failure as an item, since that is where the attribute sits.
///
/// `Error::to_compile_error` wraps the call in a block, which only parses in
/// expression position — where `html!` lives, and where this does not.
pub fn to_compile_error_item(err: &Error) -> TokenStream {
    let msg = &err.msg;
    quote::quote_spanned! { err.span => ::core::compile_error!(#msg); }
}
