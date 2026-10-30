//! The `#[job]` attribute.
//!
//! A function that runs after the response, by name, possibly in another
//! process and possibly after a restart. The attribute leaves the function as
//! written, so a test calls it directly, and gives its name a second meaning:
//!
//! ```ignore
//! #[job(attempts = 5, timeout_secs = 20)]
//! pub async fn send_receipt(order_id: i64) -> Result<(), JobError> { … }
//! ```
//!
//! becomes that function, plus a shim that reads the stored arguments, a
//! link-time `JobEntry`, and
//!
//! ```ignore
//! pub enum send_receipt {}
//!
//! impl send_receipt {
//!     pub const NAME: &'static str = "send_receipt";
//!     pub fn payload(order_id: i64) -> Result<Value, serde_json::Error> { … }
//!     pub fn request(order_id: i64) -> Result<NewJob, EnqueueError> { … }
//!     pub async fn enqueue(order_id: i64) -> Result<JobId, EnqueueError> { … }
//!     pub async fn enqueue_unique(key: &str, order_id: i64)
//!         -> Result<Option<JobId>, EnqueueError> { … }
//! }
//! ```
//!
//! An enum with no variants lives only in the type namespace, so
//! `send_receipt(1)` still calls the function while `send_receipt::enqueue(1)`
//! reaches these. That is what makes an enqueue a checked call: a misspelled
//! job, a missing argument and a wrong type are all compile errors on the call
//! site, instead of a string that fails when a worker reads it.
//!
//! Jobs live in `src/jobs.rs` or under `src/jobs/`. `rahti-build` reads those
//! files into the manifest, so a job written elsewhere would run and be
//! invisible to tooling. The location is therefore refused here, on the
//! attribute, where the fix is obvious.

use proc_macro2::{Delimiter, Ident, Literal, Span, TokenStream, TokenTree};
use quote::quote;

use crate::parse::Error;
use crate::rpc::{first_generic, head_ident, span_of, split_top_level};

#[cfg(test)]
#[path = "tests/job.rs"]
mod tests;

/// One argument the stored payload has to supply.
struct Param {
    name: Ident,
    ty: TokenStream,
}

/// What `#[job(…)]` overrides. Anything left out is the queue's policy.
#[derive(Default)]
struct Options {
    name: Option<String>,
    attempts: Option<u32>,
    timeout_secs: Option<u64>,
    backoff_ms: Option<u64>,
    max_backoff_ms: Option<u64>,
}

pub fn expand(attr: TokenStream, item: TokenStream, file: &str) -> Result<TokenStream, Error> {
    let options = parse_attr(attr)?;

    let toks: Vec<TokenTree> = item.into_iter().collect();
    let keyword = toks
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "fn"))
        .ok_or_else(|| Error {
            span: span_of(&toks),
            msg: "rahti: `#[job]` goes on a function.\n  \
                  A job is an `async fn` in `src/jobs.rs` whose parameters are \
                  what it needs to run."
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

    check_location(file, &name)?;

    let is_async = toks[..keyword]
        .iter()
        .any(|tt| matches!(tt, TokenTree::Ident(id) if id == "async"));
    if !is_async {
        return Err(Error {
            span: name.span(),
            msg: format!(
                "rahti: `{name}` is not `async`, and a job has to be.\n  \
                 Workers share a runtime, and an attempt's timeout can only \
                 stop a function that awaits."
            ),
        });
    }

    let params_at = keyword + 2;
    let params = match toks.get(params_at) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            parse_params(g.stream(), &name)?
        }
        _ => {
            return Err(Error {
                span: name.span(),
                msg: format!(
                    "rahti: `{name}` cannot be a generic `#[job]`.\n  \
                     A stored job is found by name, so the function a worker \
                     reaches has to have one signature."
                ),
            });
        }
    };

    check_return(&toks[params_at + 1..], &name)?;

    let wire = options.name.clone().unwrap_or_else(|| name.to_string());
    let vis = visibility(&toks[..keyword]);
    let shim = Ident::new(&format!("__rahti_job_{name}"), name.span());

    let reads = params.iter().map(|param| {
        let ident = &param.name;
        let ty = &param.ty;
        let key = ident.to_string();
        quote! {
            let #ident = ::rahti::jobs::argument::<#ty>(&__arguments, #wire, #key)?;
        }
    });
    let names: Vec<&Ident> = params.iter().map(|param| &param.name).collect();
    let declared: Vec<TokenStream> = params
        .iter()
        .map(|param| {
            let ident = &param.name;
            let ty = &param.ty;
            quote! { #ident: #ty }
        })
        .collect();
    let writes = params.iter().map(|param| {
        let ident = &param.name;
        let key = ident.to_string();
        quote! {
            __arguments.insert(
                #key.to_string(),
                ::rahti::serde_json::to_value(&#ident)?,
            );
        }
    });

    let attempts = optional(options.attempts.map(Literal::u32_suffixed));
    let timeout_secs = optional(options.timeout_secs.map(Literal::u64_suffixed));
    let backoff_ms = optional(options.backoff_ms.map(Literal::u64_suffixed));
    let max_backoff_ms = optional(options.max_backoff_ms.map(Literal::u64_suffixed));

    let doc = format!("Enqueue and describe the `{wire}` job, written by `#[job]`.");
    let original: TokenStream = toks.into_iter().collect();

    Ok(quote! {
        #original

        #[allow(non_snake_case)]
        fn #shim(__payload: ::rahti::serde_json::Value) -> ::rahti::jobs::JobFuture {
            ::std::boxed::Box::pin(async move {
                let __arguments = ::rahti::jobs::arguments(__payload, #wire)?;
                #(#reads)*
                ::rahti::jobs::IntoJobResult::into_job_result(#name(#(#names),*).await)
            })
        }

        ::rahti::inventory::submit! {
            ::rahti::jobs::JobEntry {
                name: #wire,
                source: ::core::concat!(::core::file!(), ":", ::core::line!()),
                attempts: #attempts,
                timeout_secs: #timeout_secs,
                backoff_ms: #backoff_ms,
                max_backoff_ms: #max_backoff_ms,
                handler: #shim,
            }
        }

        #[doc = #doc]
        #[allow(non_camel_case_types, dead_code)]
        #vis enum #name {}

        #[allow(dead_code)]
        impl #name {
            /// The stable name this job is stored and scheduled under.
            pub const NAME: &'static str = #wire;

            /// The stored payload, for a schedule to submit.
            pub fn payload(
                #(#declared),*
            ) -> ::core::result::Result<::rahti::serde_json::Value, ::rahti::serde_json::Error> {
                let mut __arguments = ::rahti::serde_json::Map::new();
                #(#writes)*
                ::core::result::Result::Ok(::rahti::serde_json::Value::Object(__arguments))
            }

            /// This job as a value, for a store to insert inside a transaction.
            pub fn request(
                #(#declared),*
            ) -> ::core::result::Result<::rahti::jobs::NewJob, ::rahti::jobs::EnqueueError> {
                let __payload = Self::payload(#(#names),*)
                    .map_err(::rahti::jobs::EnqueueError::Serialize)?;
                ::rahti::jobs::NewJob::new(#wire, __payload)
                    .map_err(::rahti::jobs::EnqueueError::Serialize)
            }

            /// Hand this job to the installed queue.
            pub async fn enqueue(
                #(#declared),*
            ) -> ::core::result::Result<::rahti::jobs::JobId, ::rahti::jobs::EnqueueError> {
                ::rahti::jobs::queue()?.submit(Self::request(#(#names),*)?).await
            }

            /// Hand this job to the installed queue unless a pending or
            /// running job holds `key`, which answers `None`.
            pub async fn enqueue_unique(
                key: &str,
                #(#declared),*
            ) -> ::core::result::Result<
                ::core::option::Option<::rahti::jobs::JobId>,
                ::rahti::jobs::EnqueueError,
            > {
                ::rahti::jobs::queue()?
                    .submit_now(Self::request(#(#names),*)?.unique(key))
                    .await
            }
        }
    })
}

fn optional(value: Option<Literal>) -> TokenStream {
    match value {
        Some(value) => quote! { ::core::option::Option::Some(#value) },
        None => quote! { ::core::option::Option::None },
    }
}

/// `src/jobs.rs`, or anything under a `jobs` directory. An empty path is a
/// tool that expanded the macro without a file, which has nothing to check.
fn check_location(file: &str, name: &Ident) -> Result<(), Error> {
    let normalized = file.replace('\\', "/");
    let mut segments = normalized.rsplit('/');
    let last = segments.next().unwrap_or_default();
    if file.is_empty() || last == "jobs.rs" || segments.any(|segment| segment == "jobs") {
        return Ok(());
    }

    Err(Error {
        span: name.span(),
        msg: format!(
            "rahti: `{name}` is a `#[job]` in {normalized}, and jobs live in \
             `src/jobs.rs` or under `src/jobs/`.\n  \
             The build reads those files into the manifest, which is how \
             tooling knows what work exists. Move the function there, or run \
             `cargo rahti generate job {name}`."
        ),
    })
}

/// The tokens before `fn` that say who can see it: `pub`, `pub(crate)`, or
/// nothing. The enum takes the function's own visibility.
fn visibility(head: &[TokenTree]) -> TokenStream {
    let Some(at) = head
        .iter()
        .position(|tt| matches!(tt, TokenTree::Ident(id) if id == "pub"))
    else {
        return TokenStream::new();
    };
    let mut vis = TokenStream::new();
    vis.extend([head[at].clone()]);
    if let Some(TokenTree::Group(g)) = head.get(at + 1)
        && g.delimiter() == Delimiter::Parenthesis
    {
        vis.extend([head[at + 1].clone()]);
    }
    vis
}

/// `#[job]`, or `#[job(attempts = 5, timeout_secs = 20, …)]`.
fn parse_attr(attr: TokenStream) -> Result<Options, Error> {
    let mut options = Options::default();
    for group in split_top_level(attr) {
        let span = span_of(&group);
        let [
            TokenTree::Ident(key),
            TokenTree::Punct(equals),
            TokenTree::Literal(value),
        ] = group.as_slice()
        else {
            return Err(attr_error(span));
        };
        if equals.as_char() != '=' {
            return Err(attr_error(span));
        }

        let text = value.to_string();
        match key.to_string().as_str() {
            "name" => {
                let wire = text
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .filter(|wire| valid_name(wire))
                    .ok_or_else(|| Error {
                        span: value.span(),
                        msg: "rahti: a job `name` is a string of 1-64 ASCII letters, \
                              digits, `-`, `_` or `.`."
                            .into(),
                    })?;
                options.name = Some(wire.to_string());
            }
            "attempts" => {
                let attempts = positive(&text, value.span(), "attempts")?;
                options.attempts = Some(u32::try_from(attempts).map_err(|_| Error {
                    span: value.span(),
                    msg: "rahti: job `attempts` does not fit in a `u32`.".into(),
                })?);
            }
            "timeout_secs" => {
                options.timeout_secs = Some(positive(&text, value.span(), "timeout_secs")?);
            }
            "backoff_ms" => {
                options.backoff_ms = Some(positive(&text, value.span(), "backoff_ms")?);
            }
            "max_backoff_ms" => {
                options.max_backoff_ms = Some(positive(&text, value.span(), "max_backoff_ms")?);
            }
            _ => return Err(attr_error(key.span())),
        }
    }
    Ok(options)
}

fn positive(text: &str, span: Span, key: &str) -> Result<u64, Error> {
    text.replace('_', "")
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| Error {
            span,
            msg: format!("rahti: job `{key}` is a whole number greater than zero."),
        })
}

fn attr_error(span: Span) -> Error {
    Error {
        span,
        msg: "rahti: `#[job]` takes `name = \"…\"`, `attempts`, `timeout_secs`, \
              `backoff_ms` and `max_backoff_ms`, or nothing.\n  \
              Anything left out is the queue's policy, and the name defaults \
              to the function's."
            .into(),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn parse_params(stream: TokenStream, func: &Ident) -> Result<Vec<Param>, Error> {
    let mut params = Vec::new();

    for group in split_top_level(stream) {
        // `self`, `&self`, `mut self` and `&mut self` all land here.
        let receiver = group
            .iter()
            .take_while(|tt| !matches!(tt, TokenTree::Punct(p) if p.as_char() == ':'))
            .find_map(|tt| match tt {
                TokenTree::Ident(id) if id == "self" => Some(id.span()),
                _ => None,
            });
        if let Some(span) = receiver {
            return Err(Error {
                span,
                msg: format!(
                    "rahti: `{func}` takes `self`, so it belongs to a type and \
                     cannot be a `#[job]`.\n  \
                     A worker finds a job by name, with nothing to call it on."
                ),
            });
        }

        let mut toks = group.into_iter();

        let name = match toks.next() {
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
                        "rahti: every argument of `{func}` needs a plain name, \
                         because the stored payload is keyed by it."
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

        if matches!(
            ty.clone().into_iter().next(),
            Some(TokenTree::Punct(ref p)) if p.as_char() == '&'
        ) {
            return Err(Error {
                span: name.span(),
                msg: format!(
                    "rahti: `{name}` in `{func}` is borrowed, and a job argument \
                     is rebuilt from a stored payload.\n  \
                     Take it by value — `String` rather than `&str`, `Vec<T>` \
                     rather than `&[T]`."
                ),
            });
        }

        params.push(Param { name, ty });
    }

    Ok(params)
}

/// Nothing, `()`, or a `Result` of `()`. A returned value would be dropped by
/// a worker that has nobody to hand it to.
fn check_return(rest: &[TokenTree], name: &Ident) -> Result<(), Error> {
    let arrow = rest.windows(2).position(|pair| {
        matches!(&pair[0], TokenTree::Punct(p) if p.as_char() == '-')
            && matches!(&pair[1], TokenTree::Punct(p) if p.as_char() == '>')
    });
    let Some(arrow) = arrow else {
        return Ok(());
    };

    let ty: Vec<TokenTree> = rest[arrow + 2..]
        .iter()
        .take_while(|tt| {
            !matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                && !matches!(tt, TokenTree::Ident(id) if id == "where")
        })
        .cloned()
        .collect();

    if is_unit(&ty)
        || (head_ident(&ty).as_deref() == Some("Result") && is_unit(&first_generic(&ty)))
    {
        return Ok(());
    }

    Err(Error {
        span: name.span(),
        msg: format!(
            "rahti: `{name}` returns a value, and a job has nobody to hand one \
             to.\n  \
             Return nothing, `Result<(), JobError>`, or `rahti::Result<()>`, \
             and write what it produced to the database."
        ),
    })
}

fn is_unit(ty: &[TokenTree]) -> bool {
    matches!(
        ty,
        [TokenTree::Group(g)]
            if g.delimiter() == Delimiter::Parenthesis && g.stream().is_empty()
    )
}
