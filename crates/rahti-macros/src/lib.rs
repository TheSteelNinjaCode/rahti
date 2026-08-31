//! The `html!` macro and the `#[component]` attribute.
//!
//! `html!` compiles HTML written inline in Rust into a single `String` build.
//! Static markup is folded into one `push_str` per run, so a page with no
//! interpolation costs exactly one allocation and one copy.
//!
//! `#[component]` marks a function as a component and names the markup it
//! returns, which is what lets a page be assembled out of pieces instead of
//! written as one block.
//!
//! Generated code refers to `::rahti::{Html, Render}` — an absolute path, so
//! it resolves in any crate that depends on `rahti`, not only in the app.
//! That is what lets components ship as an ordinary library.

mod attribute;
mod component;
mod parse;
mod rpc;
mod socket;

#[cfg(test)]
#[path = "tests/emit.rs"]
mod lib_tests;

use parse::{COMPONENT_ATTR, OWNER_ATTR, Op, Parsed, SlotKind, TagAttr};
use proc_macro2::TokenStream;
use quote::quote;

/// Build an `Html` value from inline markup.
///
/// ```ignore
/// html! {
///     <main class="prose">
///         <h1>@{title}</h1>
///         <p>"Seen "{views}" times"</p>
///         <slot />
///     </main>
/// }
/// ```
///
/// - `"text"` — the convention: text content is a quoted string. The contents
///   land exactly as written, spacing included. Character references such as
///   `&amp;` are left alone; a bare `&` is escaped. A brace reference —
///   `&#123;`, `&lbrace;` and the rest — is sent doubled, as `&amp;#123;`, so
///   the parser leaves it standing and the client runtime reads a reference
///   rather than the start of one of its expressions.
/// - `text` — the same text written plainly, as in HTML, is also accepted:
///   indentation is dropped and a space between words or beside a tag is kept,
///   the way JSX treats it. Prefer the quoted form. Unlike JSX text, this goes
///   through the Rust lexer first, so an apostrophe, a `"` or an arrow is a
///   compile error before the macro sees it, and rewrapping a long line can
///   change the spacing that reaches the page.
/// - `{expr}` — the client runtime's. The macro does not read it: the source
///   is copied through and escaped as text, so the runtime finds the
///   expression the author wrote when it reads the element back off the page.
/// - `@{expr}` — Rust, rendered here. Any value implementing
///   `::rahti::Render`. Escaped in both text and attribute position; an
///   `Html` value passes through raw.
/// - `<slot />` — expands to the `children` binding in scope, which is what
///   makes a `layout.rs` a layout.
/// - `<Card title="…">…</Card>` — a tag starting with a capital letter calls
///   the `#[component]` function of that name. Props are its parameters,
///   written as literals or `@{expr}`; the children between the tags become
///   its `children` argument, and a self-closing tag passes none.
/// - `<>…</>` — a fragment: several sibling roots for one block, rendered
///   behind a wrapper element whose `display: contents` keeps it out of the
///   layout. Allowed as a block's root and as the root of a component tag's
///   children; inside an element, siblings need no wrapper.
/// - `<script>` and `<style>` hold text, not markup. Their bodies are emitted
///   as written — no escaping, and no `{…}`, which in JavaScript is a block
///   and in CSS is a rule. `@{expr}` inside `<script>` is the one exception:
///   the Rust expression is rendered as JavaScript source through
///   `::rahti::RenderJs`, so a page can seed `pp.state(…)` with values known
///   at render rather than fetching them again. `<style>` keeps the raw rule.
///
/// The split follows which side owns the value. A page's own data is known
/// when it renders and is `@{…}`; anything the client runtime holds — state
/// declared in a `<script>`, a value it computes — stays `{…}` and is still
/// there in the HTML for the runtime to bind.
///
/// In a convention file under `app/`, the single parent element is stamped
/// with `pp-component="…"` naming the file it came from. A block whose single
/// parent is `<html>` — a root layout — is named differently: the attribute
/// would make the document itself a component, so a `<template>` carrying it
/// is injected around `<slot />` instead. Writing a `pp-component` anywhere in
/// the block turns both off and leaves the marker where the author put it.
#[proc_macro]
pub fn html(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let file = proc_macro::Span::call_site().file();

    match parse::parse(input.into()) {
        Ok(parsed) => emit(stamp(parsed, &file)).into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Turn a function into a component.
///
/// ```ignore
/// #[component]
/// pub fn UserCard(name: &str, children: Html) -> Html {
///     html! {
///         <article class="user-card">
///             <h2>@{name}</h2>
///             <slot />
///         </article>
///     }
/// }
/// ```
///
/// The function takes its props as ordinary arguments and returns the single
/// parent element `html!` already requires. In markup it is called as a tag —
/// `<UserCard name="Ada"><Badge text="Rust" /></UserCard>` — where the props
/// are named attributes and whatever sits between the tags becomes the
/// `children: Html` argument, rendered by `<slot />`. It is still an ordinary
/// function, which is the form expression contexts use:
/// `@{Html::concat(users.iter().map(|u| UserCard(u, Html::empty())))}`.
///
/// The element that comes back is stamped `pp-component="user_card_3b91f0ae"`
/// — the function name, plus a hash of where it is defined — which is how the
/// client runtime tells one block of markup from another. Writing
/// `pp-component` yourself in the markup overrides it.
#[proc_macro_attribute]
pub fn component(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let file = proc_macro::Span::call_site().file();
    let original: TokenStream = item.clone().into();

    match attribute::expand(attr.into(), item.into(), &file) {
        Ok(expanded) => expanded.into(),
        Err(err) => {
            // Emit the function alongside the error: whatever else refers to
            // it still resolves, so the one real problem is the one reported.
            let mut out = attribute::to_compile_error_item(&err);
            out.extend(original);
            out.into()
        }
    }
}

/// Let the browser call this function.
///
/// ```ignore
/// #[rpc]
/// pub async fn greet(name: String) -> Greeting {
///     Greeting { message: format!("Hello, {name}") }
/// }
/// ```
///
/// From a page's `<script>`, that function is one call away:
///
/// ```js
/// const greeting = await pp.rpc("greet", { name: "Ada" });
/// ```
///
/// The client POSTs to the URL the page is already on, so an `#[rpc]` belongs
/// beside the markup that calls it: in the `page.rs` whose URL it is, or in
/// the `src/components/` file that owns the button. `rahti-build` finds it in
/// either place and gives that URL a POST handler that dispatches by name.
///
/// A component has no URL of its own — it renders wherever it is called — so
/// its rpcs go in one dispatcher every page falls through to. That makes the
/// names app-wide: two components cannot share one, and neither can a
/// component and a page.
///
/// Arguments are read out of the payload by the parameter's own name, so they
/// have to be owned types serde can build: `String`, not `&str`. Returning
/// `rahti::Result<T>` is the fallible form — an `Err` reaches the browser as
/// its status and its message, which is what `pp.rpc` throws.
///
/// Two parameter and return types change which wire the call uses, because
/// the client changes wires too:
///
/// - `avatar: RpcFile`, `Option<RpcFile>` or `Vec<RpcFile>` is an upload. A
///   `File` anywhere in the payload makes `pp.rpc` post `FormData` instead of
///   JSON, and the values beside the file come across as text and are read
///   back into their parameters.
/// - `-> RpcStream` sends the answer in pieces, as `text/event-stream`, which
///   the client reads through the `onStream` option rather than by awaiting
///   one value.
///
/// The function itself is left alone. It is still an ordinary function, and
/// the page that declares it can call it while rendering.
#[proc_macro_attribute]
pub fn rpc(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let original: TokenStream = item.clone().into();

    match rpc::expand(attr.into(), item.into()) {
        Ok(expanded) => expanded.into(),
        Err(err) => {
            // The function still compiles, so the error reported is the one
            // that matters rather than every call site as well.
            let mut out = attribute::to_compile_error_item(&err);
            out.extend(original);
            out.into()
        }
    }
}

/// Let the browser hold a conversation with this function, over a WebSocket.
///
/// ```ignore
/// use rahti::ws::Socket;
///
/// #[socket]
/// pub async fn chat(room: String, mut socket: Socket) -> rahti::Result<()> {
///     while let Some(message) = socket.recv::<String>().await? {
///         socket.send(&message).await;
///     }
///     Ok(())
/// }
/// ```
///
/// From a page's `<script>`:
///
/// ```js
/// const sock = pp.socket("chat", { room: "lobby" }, { onMessage: m => … });
/// sock.send("hello");
/// ```
///
/// Every parameter before the `Socket` arrives in the connection's first
/// frame, read by the parameter's own name — the same payload rules as
/// `#[rpc]`, owned types included. The `Socket` has to be last, the function
/// has to be `async`, and it returns nothing or `rahti::Result<()>`: a socket
/// answers through its frames, not through a return value. `#[socket(auth)]`
/// refuses the handshake with a 401 unless the browser carries a session.
///
/// Socket names are application-wide wherever the function is written — a
/// page, a component, a library — because every connection arrives on one
/// endpoint and is dispatched by name alone.
///
/// Requires the `ws` feature of `rahti`:
/// `rahti = { version = "…", features = ["ws"] }` — which is what
/// `cargo rahti new --ws` writes.
#[proc_macro_attribute]
pub fn socket(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let original: TokenStream = item.clone().into();

    match socket::expand(attr.into(), item.into()) {
        Ok(expanded) => expanded.into(),
        Err(err) => {
            // The function still compiles, so the error reported is the one
            // that matters rather than every call site as well.
            let mut out = attribute::to_compile_error_item(&err);
            out.extend(original);
            out.into()
        }
    }
}

/// Write `pp-component="…"` into the root element's open tag — or, for a
/// document, around its slot.
fn stamp(mut parsed: Parsed, file: &str) -> Vec<Op> {
    let file_id = component::identifier(file);
    // The scope children between component tags read is the block they were
    // written in — the page named after the file, or the `#[component]`
    // function whose name was handed down. That is the id an owner wrap
    // carries; static child blocks are still named by the file alone.
    let owner_id = parsed.forced_component.clone().or_else(|| file_id.clone());

    // Children written between component tags are their own blocks, named
    // the way an inline `html! { … }` child argument is: by the file alone.
    // That keeps the two spellings byte-identical — and it is independent of
    // how this block itself is named, so it happens before any early return.
    stamp_component_children(&mut parsed.ops, file_id.as_deref(), owner_id.as_deref());

    if parsed.declares_component {
        return parsed.ops;
    }
    // A name handed down by `#[component]` wins: it knows which function the
    // block was written in, which is more than the file path can say.
    let Some(id) = parsed
        .forced_component
        .take()
        .or_else(|| component::identifier(file))
    else {
        return parsed.ops;
    };

    if parsed.renders_document {
        return wrap_slot(parsed.ops, parsed.slot_at, &id);
    }

    let Some(slot) = parsed.component_slot else {
        return parsed.ops;
    };
    write_name(&mut parsed.ops, slot, &id);
    parsed.ops
}

/// Write a block's name where its slot says: as a `pp-component` attribute
/// in a root element's open tag, or as the text of a fragment's open marker.
fn write_name(ops: &mut [Op], (index, offset, kind): (usize, usize, SlotKind), id: &str) {
    if let Some(Op::Static(markup)) = ops.get_mut(index) {
        match kind {
            SlotKind::Element => {
                markup.insert_str(offset, &format!(" {COMPONENT_ATTR}=\"{id}\""));
            }
            SlotKind::Fragment => {
                markup.insert_str(offset, id);
            }
        }
    }
}

/// Put the block's name on a `<template>` around `<slot />`.
///
/// The single parent element of a root layout is `<html>`, and stamping the
/// document element would tell the client runtime that the whole document is
/// a component to re-render — `<head>` and all. So a document is named where
/// the reactive part of it actually starts: the children it wraps. The
/// `<template>` renders nothing itself, which is what keeps the boundary out
/// of the page's own layout.
///
/// A document with no slot renders no children, so there is nothing to own
/// and nothing is injected.
fn wrap_slot(mut ops: Vec<Op>, slot_at: Option<usize>, id: &str) -> Vec<Op> {
    let Some(at) = slot_at else {
        return ops;
    };
    ops.insert(
        at,
        Op::Static(format!("<template {COMPONENT_ATTR}=\"{id}\">")),
    );
    ops.insert(at + 2, Op::Static("</template>".into()));
    ops
}

/// Name the child blocks of every component tag, recursively — and hand the
/// ones that read this block's state to their owner.
///
/// Three shapes, decided per child block:
///
/// - **Owner-scoped children** — the block carries a client `{…}` binding
///   (in text or on an element's attribute) and no `<script>` of its own.
///   Those expressions read the state of the block that wrote them, the way
///   React children close over the component that rendered them — so the
///   whole child block ships as `<template pp-owner="…">…</template>` inside
///   the component's boundary. The client runtime extracts the template,
///   renders it in the owner's scope, and re-renders it whenever the owner's
///   state changes. No `pp-component` is stamped: the content is the
///   owner's, not a block of its own.
/// - **Self-contained children** — the block carries its own `<script>`. It
///   is a reactive block of its own wherever it lands, so a single root gets
///   `pp-component`, exactly where a separate `html! { … }` child block
///   would have carried it.
/// - **Static children** — no bindings at all. Served inline as before,
///   visible without JavaScript, with a single root named for form's sake.
///
/// A fragment-rooted child block keeps the old stamping even when it carries
/// bindings: its comment markers are what the runtime raises into a
/// boundary, and that shape has a name slot rather than an element to wrap.
/// An author-written `pp-component` inside the children turns every
/// injection off, the same override every block has.
fn stamp_component_children(ops: &mut [Op], file_id: Option<&str>, owner_id: Option<&str>) {
    for op in ops {
        let Op::Component(call) = op else { continue };
        let Some(child) = &mut call.children else {
            continue;
        };
        stamp_component_children(&mut child.ops, file_id, owner_id);
        if child.declares_component {
            continue;
        }

        let is_fragment = matches!(child.component_slot, Some((_, _, SlotKind::Fragment)));
        if child.has_bindings && !child.has_script && !is_fragment {
            if let Some(owner) = owner_id {
                child
                    .ops
                    .insert(0, Op::Static(format!("<template {OWNER_ATTR}=\"{owner}\">")));
                child.ops.push(Op::Static("</template>".into()));
                continue;
            }
        }

        let (Some(id), Some(slot)) = (file_id, child.component_slot) else {
            continue;
        };
        write_name(&mut child.ops, slot, id);
    }
}

fn emit(ops: Vec<Op>) -> TokenStream {
    let mut body = TokenStream::new();
    let mut pending = String::new();
    let mut statics = 0usize;

    for op in ops {
        if let Op::Static(s) = &op {
            pending.push_str(s);
            continue;
        }
        statics += flush(&mut pending, &mut body);
        match op {
            Op::Static(_) => unreachable!("handled above"),
            Op::Text(expr) => body.extend(quote! {
                ::rahti::Render::render_into(&(#expr), &mut __rahti);
            }),
            // `name=@{expr}` — the whole attribute, because the value is what
            // says whether there is one. `href=@{None}` writes nothing, and a
            // `false` on an attribute HTML reads by presence writes nothing
            // either; see `rahti::attribute` and the boolean table in
            // `parse.rs`.
            Op::Attr {
                name,
                expr,
                boolean,
            } => body.extend(quote! {
                ::rahti::attribute(#name, &(#expr), #boolean, &mut __rahti);
            }),
            // The attributes of a tag carrying `@{..spread}`, assembled so a
            // later one replaces an earlier one rather than being written
            // beside it.
            Op::OpenTag(attrs) => {
                let mut writes = TokenStream::new();
                for attr in attrs {
                    writes.extend(match attr {
                        TagAttr::Literal { name, value } => {
                            // A bare boolean attribute has no value to carry,
                            // and `name=""` is how HTML spells the same thing.
                            let value = value.unwrap_or_default();
                            quote! { __rahti_attrs.set(#name, #value); }
                        }
                        TagAttr::Value {
                            name,
                            expr,
                            boolean,
                        } => quote! { __rahti_attrs.value(#name, &(#expr), #boolean); },
                        TagAttr::Spread(expr) => quote! { __rahti_attrs.spread(&(#expr)); },
                    });
                }
                body.extend(quote! {
                    {
                        let mut __rahti_attrs = ::rahti::AttrList::new();
                        #writes
                        __rahti_attrs.write_into(&mut __rahti);
                    }
                });
            }
            // `@{…}` inside a `<script>`. The script side has its own trait
            // rather than reusing `Render`: JS source has no HTML escaping,
            // and a string that renders bare in markup needs quoting in a
            // script. See `rahti::RenderJs`.
            Op::ScriptText(expr) => body.extend(quote! {
                ::rahti::RenderJs::render_js_into(&(#expr), &mut __rahti);
            }),
            // `children` is a plain binding, so a `<slot />` outside a layout
            // fails with rustc's ordinary "cannot find value" error.
            Op::Slot => body.extend(quote! {
                ::rahti::Render::render_into(&children, &mut __rahti);
            }),
            // `<Card …>` — a call through the named-props companion that
            // `#[component]` generates beside the function: a struct literal
            // sharing the function's name takes the props in written order,
            // and its `__rahti_call` — which knows the declaration order —
            // hands them to the function. The tag's children become the
            // `children` field, built by this same emitter; a self-closing
            // tag passes no such field at all.
            //
            // The literal ends in `..Default::default()`, which is what
            // makes a prop optional: `<Button variant="outline">` fills the
            // one it names and takes the component's defaults for the rest,
            // the way a React component reads `{ variant = "default" }` out
            // of its props. The call site cannot do this for itself — it
            // deliberately does not know the parameter list, which is the
            // whole reason the companion exists — so the struct's own
            // `Default` supplies the omitted fields.
            //
            // The cost is that a forgotten prop is no longer "missing
            // field": it compiles, and renders the default. The diagnostic
            // that remains is for a prop whose type has no `Default`, which
            // reports as an unsatisfied trait bound at the tag.
            Op::Component(call) => {
                let name = call.name;
                let mut fields = TokenStream::new();
                for (prop, value) in call.props {
                    fields.extend(quote! { #prop: #value, });
                }
                // Every `{…}` prop, as source, in one field. A component
                // takes them with `bindings: &[(&str, &str)]` and writes
                // them onto whatever element it renders, which is what puts
                // `<Button onclick={save()}>` on the button itself.
                //
                // Only emitted when the tag wrote one, so a component that
                // does not declare `bindings` is unaffected until somebody
                // hands it a client expression — and then says so, as an
                // unknown field at the tag.
                if !call.bindings.is_empty() {
                    let entries = call.bindings.iter().map(|(name, expr)| {
                        quote! { (#name, #expr), }
                    });
                    // Spelled out as a slice. The props struct is generic
                    // over every field, so there is no slice type for the
                    // array literal to coerce towards and inference would
                    // otherwise pin the field to `&[_; N]`.
                    fields.extend(quote! {
                        bindings: &[#(#entries)*]
                            as &'static [(&'static str, &'static str)],
                    });
                }
                if let Some(child) = call.children {
                    let block = emit(child.ops);
                    fields.extend(quote! { children: #block, });
                }
                body.extend(quote! {
                    ::rahti::Render::render_into(
                        &(#name {
                            #fields
                            ..::core::default::Default::default()
                        }
                        .__rahti_call(#name)),
                        &mut __rahti,
                    );
                });
            }
        }
    }
    statics += flush(&mut pending, &mut body);

    // Every byte of static markup is known here; skip the regrowth.
    let hint = statics;
    quote! {{
        let mut __rahti = ::std::string::String::with_capacity(#hint);
        #body
        ::rahti::Html::from_raw(__rahti)
    }}
}

/// Move accumulated static markup into one `push_str`. Returns its length.
fn flush(pending: &mut String, body: &mut TokenStream) -> usize {
    if pending.is_empty() {
        return 0;
    }
    let chunk = std::mem::take(pending);
    let len = chunk.len();
    body.extend(quote! { __rahti.push_str(#chunk); });
    len
}
