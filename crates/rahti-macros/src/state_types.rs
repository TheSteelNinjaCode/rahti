//! What a block's state holds, worked out from what is put into it.
//!
//! A `pp.state` value has no declared type. When everything a block ever
//! puts into one is the response of the same `#[rpc]` — or `[]`, `null` and
//! the like, which contradict nothing — the state holds that response, and
//! a field read off it in the script, in a `{binding}` or in a `pp-for` row
//! can be followed through the response's shape:
//!
//! ```ignore
//! <template pp-for="domain in domains">
//!     <li key={domain.id}>{domain.nmae}</li>      // no such field
//! </template>
//! <script>r##"
//!     const [domains, setDomains] = pp.state([]);
//!     async function load() {
//!         setDomains(await pp.rpc("list_domains", {}));
//!     }
//! "##</script>
//! ```
//!
//! A state gets a shape only when nothing leaves room for doubt. One setter
//! call with anything else, a setter handed to other code, a field written
//! in place — and the state has no shape and none of its reads are looked at.
//!
//! A state that starts from a server value, `pp.state(@{…})`, holds that
//! value until it is given something else, and the macro cannot see what a
//! server value is. The compiler can (see `rahti::shape`). So a read the
//! response does not have is not refused here: it becomes a [`SeedProbe`], a
//! question written around the server value, which is an error only when
//! the server value's own struct lacks the field too. A state that is given
//! nothing but the server value it starts from is read as that value is.
//!
//! [`check`] refuses a read that names a field the state's shape does not
//! have. It is as careful about a read as about a state: a field name the
//! block assigns anywhere (`row.open = true`) is never refused, because a
//! value reached through an alias can be given that field in place.
//!
//! With `RAHTI_STATE_REPORT` set to a file's path, `html!` also appends what
//! each block's states and reads came to, one tab-separated line each:
//! (a server value's reads are decided by the compiler: see `rahti::shape`)
//!
//! ```text
//! S  file  state  typed|untyped  shape or reason  detail
//! R  file  place  expression     known|open|missing  detail
//! L  file  directive             typed|untyped  what it loops over
//! D  file  rpc                   known|open|missing   (a read straight off a result)
//! P  file  none|written|followed|open|unknown          (a `pp.rpc` payload's keys)
//! V  file  place  expression     server constant        (a read the compiler decides)
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};

use rahti_js::{Code, Derived, Origin, Step, Use};

use crate::parse::{ClientKind, ClientSource, Error};
use crate::rpc_shapes::{Shape, Shapes, Walk};
use std::collections::BTreeSet;

/// A read that is refused if the compiler finds a script's server value to
/// be an rpc's response.
pub struct SeedProbe {
    /// The `pp-script` key of the script the server value is written in.
    pub script: String,
    /// Which of that script's `@{…}` values, counted from zero.
    pub index: usize,
    /// A block to run beside that value, which names it [`seed_ident`].
    pub tokens: TokenStream,
}

/// The name a [`SeedProbe`] knows the server value by.
pub fn seed_ident() -> proc_macro2::Ident {
    format_ident!("__rahti_seed")
}

/// Refuses the first read, in this block's script or markup, of a field its
/// state does not hold, and returns the reads only the compiler can refuse.
pub fn check(sources: &[ClientSource]) -> Result<Vec<SeedProbe>, Error> {
    let Some(shapes) = Shapes::current() else {
        return Ok(Vec::new());
    };
    let crate_wide = Shapes::crate_wide().filter(|shapes| shapes.has_structs());
    check_against(sources, &shapes, crate_wide.as_deref().map(Shapes::fields))
}

/// [`check`], against the shapes of the file the block is in, and the field
/// names the crate's generated shapes know — `None` when the crate has no
/// generated shapes, and a server value's reads are then left alone.
pub fn check_against(
    sources: &[ClientSource],
    shapes: &Shapes,
    fields: Option<&BTreeSet<String>>,
) -> Result<Vec<SeedProbe>, Error> {
    let mut probes = Vec::new();
    let script = sources
        .iter()
        .find(|source| source.kind == ClientKind::Script && !source.foreign)
        .map(|script| script.key.clone());
    for finding in analyze(sources, shapes) {
        if let Finding::ServerRead {
            source,
            root,
            from,
            index,
            before,
            path,
            last_only,
            ..
        } = &finding
        {
            let (Some(script), Some(fields)) = (&script, fields) else {
                continue;
            };
            let locator = crate::rpc_calls::Locator::new(&sources[*source]);
            probes.push(SeedProbe {
                script: script.clone(),
                index: *index,
                tokens: server_read(fields, &locator, root, from, *before, path, *last_only),
            });
            continue;
        }
        let Finding::Read {
            source,
            outcome:
                Outcome::Missing {
                    owner,
                    fields,
                    field,
                    span,
                    state,
                    rpc,
                },
            ..
        } = finding
        else {
            continue;
        };
        let locator = crate::rpc_calls::Locator::new(&sources[source]);
        return Err(Error {
            span: locator.span(&span),
            msg: crate::authoring::state_field(
                &state,
                &rpc,
                &owner,
                &field,
                &fields,
                &locator.line_at(span.start),
            ),
        });
    }
    Ok(probes)
}

/// The module `rahti::shapes!` expands to, beside the routes: a marker type
/// for each object the crate's structs are made of, `rahti::shape::Shape`
/// for each struct, and a trait for each field name, implemented for the
/// objects that have it.
pub fn crate_shapes(shapes: &Shapes) -> TokenStream {
    let level = shapes.type_level();
    let markers = level.objects.iter().map(|(marker, _)| {
        quote! {
            pub struct #marker;
            // A computed key reads anything.
            impl ::rahti::shape::Element for #marker {
                type Out = ::rahti::shape::Open;
            }
        }
    });
    let structs = level.named.iter().map(|(path, node)| {
        quote! {
            impl ::rahti::shape::Shape for #path {
                type Node = #node;
            }
        }
    });
    let fields = shapes.fields().iter().map(|field| {
        let name = Shapes::field_trait(field);
        let has = level.objects.iter().filter_map(|(marker, fields)| {
            let (_, node) = fields.iter().find(|(known, _)| known == field)?;
            Some(quote! {
                impl #name for #marker {
                    type Out = #node;
                }
            })
        });
        quote! {
            pub trait #name {
                type Out;
            }
            impl #name for ::rahti::shape::Open {
                type Out = ::rahti::shape::Open;
            }
            impl<T> #name for ::rahti::shape::Arr<T> {
                type Out = ::rahti::shape::Open;
            }
            impl<T: #name> #name for ::rahti::shape::Maybe<T> {
                type Out = T::Out;
            }
            #(#has)*
        }
    });
    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code, clippy::all)]
        pub mod __rahti_shapes {
            #(#markers)*
            #(#structs)*
            #(#fields)*
        }
    }
}

/// One read of a server value, as questions for the compiler: one per field
/// step, each of which is an error only when the value reaches the object
/// that step reads and the object has no such field. An earlier step that
/// fails says so itself, and a value whose type has no shape says nothing.
///
/// The three answers are told apart by method resolution, which tries a
/// receiver one reference at a time: "has the field" is found first, "reaches
/// the object" second, and "an earlier step failed" last.
fn server_read(
    fields: &BTreeSet<String>,
    locator: &crate::rpc_calls::Locator,
    root: &str,
    from: &From,
    before: usize,
    path: &[Step],
    last_only: bool,
) -> TokenStream {
    let name = match from {
        From::Constant(name) | From::State(name) | From::SeededState(name, _) => name.as_str(),
    };
    let literal = |text: String| text.replace('{', "{{").replace('}', "}}");
    let seed = seed_ident();
    let mut out = TokenStream::new();
    let mut bounds: Vec<TokenStream> = Vec::new();
    let mut current = quote! { <S as ::rahti::shape::Shape>::Node };
    let mut owner = root.to_string();
    for (position, step) in path.iter().enumerate() {
        match step {
            Step::Index => {
                bounds.push(quote! { #current: ::rahti::shape::Element });
                current = quote! { <#current as ::rahti::shape::Element>::Out };
                if position >= before {
                    owner.push_str("[]");
                }
            }
            Step::Field { name: field, span } => {
                // A field some struct has is that field's trait; one none
                // has is the trait only an open shape answers to.
                let has = if fields.contains(field) {
                    let has = Shapes::field_trait(field);
                    quote! { crate::routes::__rahti_shapes::#has }
                } else {
                    quote! { ::rahti::shape::NoField }
                };
                // For a state that is also given a response, only the step
                // the response lacks is in question: an earlier one the
                // response has is read from it after the first reload.
                if position >= before && (!last_only || position + 1 == path.len()) {
                    let [message, label, note] = crate::authoring::server_value_field(
                        &owner,
                        field,
                        from,
                        &locator.line_at(span.start),
                    );
                    let (message, note) = (literal(message), literal(note));
                    // Located on the field, so the error is reported there.
                    let asked = quote_spanned! {Span::call_site().located_at(locator.span(span))=>
                        __rahti_read((&&(&::rahti::shape::Probe(&#seed)).shape()).verdict())
                    };
                    out.extend(quote! {{
                        #[diagnostic::on_unimplemented(message = #message, label = #label, note = #note)]
                        trait __RahtiRead {}
                        impl __RahtiRead for ::rahti::shape::Fine {}
                        fn __rahti_read<R: __RahtiRead>(_: R) {}
                        // The object has the field, or the value has no shape.
                        trait __RahtiHas {
                            fn verdict(&self) -> ::rahti::shape::Fine {
                                ::rahti::shape::Fine
                            }
                        }
                        impl<S: ::rahti::shape::Shape + ?Sized> __RahtiHas for &::rahti::shape::Known<S>
                        where
                            #(#bounds,)*
                            #current: #has
                        {
                        }
                        impl __RahtiHas for &::rahti::shape::Unknown {}
                        // The read reaches the object, which has no such field.
                        trait __RahtiLacks<M: ?Sized> {
                            fn verdict(&self) -> ::rahti::shape::Missing<M> {
                                ::rahti::shape::Missing::new()
                            }
                        }
                        impl<S: ::rahti::shape::Shape + ?Sized> __RahtiLacks<#current>
                            for &&::rahti::shape::Known<S>
                        where
                            #(#bounds,)*
                        {
                        }
                        // An earlier step is the one that failed.
                        trait __RahtiEarlier {
                            fn verdict(&self) -> ::rahti::shape::Fine {
                                ::rahti::shape::Fine
                            }
                        }
                        impl<S: ?Sized> __RahtiEarlier for ::rahti::shape::Known<S> {}
                        #[allow(unused_imports)]
                        use ::rahti::shape::{HasShape as _, NoShape as _};
                        #[allow(clippy::needless_borrow)]
                        let () = #asked;
                    }});
                }
                if position >= before {
                    owner.push('.');
                    owner.push_str(field);
                }
                bounds.push(quote! { #current: #has });
                current = quote! { <#current as #has>::Out };
            }
        }
    }
    // With a report asked for, a read whose value has no shape is a warning
    // naming the constant, so the unchecked ones can be counted.
    if std::env::var_os("RAHTI_STATE_REPORT").is_some() && !out.is_empty() {
        let note = format!("RAHTI_UNSHAPED `{name}`");
        out.extend(quote! {{
            trait __RahtiShaped {
                fn measured(&self) {}
            }
            impl<S: ?Sized> __RahtiShaped for ::rahti::shape::Known<S> {}
            trait __RahtiUnshaped {
                #[deprecated(note = #note)]
                fn measured(&self) {}
            }
            impl __RahtiUnshaped for ::rahti::shape::Unknown {}
            #[allow(unused_imports)]
            use ::rahti::shape::{HasShape as _, NoShape as _};
            (&::rahti::shape::Probe(&#seed)).shape().measured();
        }});
    }
    out
}

/// Appends this block's findings to the report, when one is asked for.
pub fn report(sources: &[ClientSource]) {
    let Some(path) = std::env::var_os("RAHTI_STATE_REPORT") else {
        return;
    };
    let (Some(shapes), Some(file)) = (Shapes::current(), crate::rpc_shapes::current_file()) else {
        return;
    };
    let findings = analyze(sources, &shapes);
    let mut text = String::new();
    // Each `pp.rpc` payload: whether every key it sends is known.
    for source in sources {
        let code = match source.kind {
            ClientKind::Script => Code::Script,
            ClientKind::Handler => Code::Handler,
            ClientKind::Expression => Code::Expression,
        };
        for call in rahti_js::rpc_calls(code, &source.source) {
            let kind = match &call.payload {
                rahti_js::Payload::Missing => "none",
                rahti_js::Payload::Unchecked => "unknown",
                rahti_js::Payload::Object { open: true, .. } => "open",
                rahti_js::Payload::Object { keys, .. } if keys.iter().any(|key| key.followed) => {
                    "followed"
                }
                rahti_js::Payload::Object { .. } => "written",
            };
            text.push_str(&format!(
                "P	{file}	{kind}
"
            ));
        }
    }
    // Reads straight off an awaited result, which `rpc_calls` checks.
    for source in sources {
        if source.kind != ClientKind::Script {
            continue;
        }
        for read in rahti_js::rpc_reads(Code::Script, &source.source) {
            let known = match shapes.response(&read.rpc) {
                Some(response) => match shapes.walk(&response, &read.rpc, &read.steps) {
                    Walk::Known(_) => "known",
                    Walk::Open => "open",
                    Walk::Missing(_) => "missing",
                },
                None => "open",
            };
            text.push_str(&format!("D\t{file}\t{}\t{known}\n", read.rpc));
        }
    }
    for finding in &findings {
        let cells: Vec<String> = match finding {
            Finding::State {
                name,
                shape: Ok(shape),
            } => vec![
                "S".into(),
                file.clone(),
                name.clone(),
                "typed".into(),
                shape.to_string(),
            ],
            Finding::State {
                name,
                shape: Err((reason, detail)),
            } => vec![
                "S".into(),
                file.clone(),
                name.clone(),
                "untyped".into(),
                (*reason).into(),
                detail.clone(),
            ],
            Finding::Read {
                place,
                expression,
                outcome,
                ..
            } => {
                let (word, detail) = match outcome {
                    Outcome::Known => ("known", String::new()),
                    Outcome::Open => ("open", String::new()),
                    Outcome::Missing { owner, fields, .. } => {
                        ("missing", format!("{owner} has: {}", fields.join(", ")))
                    }
                };
                vec![
                    "R".into(),
                    file.clone(),
                    (*place).into(),
                    expression.clone(),
                    word.into(),
                    detail,
                ]
            }
            Finding::ServerRead {
                place,
                expression,
                from,
                ..
            } => vec![
                "V".into(),
                file.clone(),
                (*place).into(),
                expression.clone(),
                match from {
                    From::Constant(name) | From::State(name) | From::SeededState(name, _) => {
                        name.clone()
                    }
                },
                match from {
                    From::Constant(_) => "constant",
                    From::State(_) => "state",
                    From::SeededState(..) => "seeded",
                }
                .into(),
            ],
            Finding::Loop {
                directive,
                typed,
                of,
            } => vec![
                "L".into(),
                file.clone(),
                directive.clone(),
                if *typed { "typed" } else { "untyped" }.into(),
                of.clone(),
            ],
        };
        let line = cells
            .iter()
            .map(|cell| cell.replace(['\t', '\n', '\r'], " "))
            .collect::<Vec<_>>()
            .join("\t");
        text.push_str(&line);
        text.push('\n');
    }
    if let Ok(mut out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = out.write_all(text.as_bytes());
    }
}

/// One thing found about a block.
#[derive(Debug, PartialEq)]
pub enum Finding {
    /// A state, and what it holds — a response's shape, or the server value
    /// it starts from — or why that is not known.
    State {
        name: String,
        shape: Result<String, (&'static str, String)>,
    },
    /// A field read off a state that has a shape, or off a row of one.
    Read {
        /// `script`, `binding`, `handler` or `row`.
        place: &'static str,
        /// Which of the block's sources the read is written in.
        source: usize,
        expression: String,
        outcome: Outcome,
    },
    /// A field read off a server value that is not state, or off a row of
    /// one. Whether the field is there is the compiler's to say.
    ServerRead {
        place: &'static str,
        source: usize,
        /// The name the read starts at, as written: `project`.
        root: String,
        expression: String,
        /// What holds the server value, and which of the script's it is.
        from: From,
        index: usize,
        /// How many steps lead from the server value to `root`.
        before: usize,
        /// Every step from the server value to the field read.
        path: Vec<Step>,
        /// Only the last step is in question: the response a seeded state
        /// is given has every earlier one.
        last_only: bool,
    },
    /// A `pp-for`, whether its rows have a shape, and what they are rows of.
    Loop {
        directive: String,
        typed: bool,
        of: String,
    },
}

/// What a server value is read through.
#[derive(Debug, Clone, PartialEq)]
pub enum From {
    /// `const name = @{…}`.
    Constant(String),
    /// A state given nothing but the server value it starts from.
    State(String),
    /// A state that starts from a server value and is given this rpc's
    /// response.
    SeededState(String, String),
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Every field read is one the shape has.
    Known,
    /// Nothing certain can be said about the read.
    Open,
    /// The read names a field the value does not have.
    Missing {
        /// The value read from, as written: `domain.owner`.
        owner: String,
        /// The fields that value has.
        fields: Vec<String>,
        /// The field it does not have, and where in the source it is named.
        field: String,
        span: Range<usize>,
        /// The state the value belongs to, and the rpc whose response it holds.
        state: String,
        rpc: String,
    },
}

/// A `pp-for="item in items"` or `pp-for="(item, index) in items"`.
struct Loop<'a> {
    item: &'a str,
    index: Option<&'a str>,
    collection: &'a str,
}

fn parse_loop(directive: &str) -> Option<Loop<'_>> {
    let (variables, collection) = directive.split_once(" in ")?;
    let variables = variables.trim();
    let (item, index) = match variables
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        Some(pair) => match pair.split_once(',') {
            Some((item, index)) => (item.trim(), Some(index.trim())),
            None => (pair.trim(), None),
        },
        None => (variables, None),
    };
    Some(Loop {
        item,
        index,
        collection: collection.trim(),
    })
}

/// What a name means where it is written: a state of the block, a setter, or
/// a loop variable that hides either.
enum Meaning<'a> {
    State(&'a str),
    Setter(&'a str),
    /// A server value that is not state, `const name = @{…}`: its name and
    /// which of the script's server values it is.
    Server(&'a str, usize),
    /// The row of the loop at this depth, counted from the outermost.
    Row(usize),
    /// A loop's index, or a name the block's script does not declare as state.
    Nothing,
}

struct Block<'a> {
    /// Setter name to state name.
    setters: HashMap<&'a str, &'a str>,
    /// State names.
    states: Vec<&'a str>,
    /// The script's server constants, and which server value each one is.
    servers: Vec<(&'a str, usize)>,
}

impl<'a> Block<'a> {
    fn meaning(&self, name: &str, loops: &[String]) -> Meaning<'a> {
        for (depth, directive) in loops.iter().enumerate().rev() {
            if let Some(looped) = parse_loop(directive) {
                if looped.item == name {
                    return Meaning::Row(depth);
                }
                if looped.index == Some(name) {
                    return Meaning::Nothing;
                }
            }
        }
        if let Some(state) = self.states.iter().find(|state| **state == name) {
            return Meaning::State(state);
        }
        if let Some(state) = self.setters.get(name) {
            return Meaning::Setter(state);
        }
        match self.servers.iter().find(|(server, _)| *server == name) {
            Some((server, index)) => Meaning::Server(server, *index),
            None => Meaning::Nothing,
        }
    }
}

/// The state a loop's rows come from, when its collection is a plain path
/// from one: `domains`, `page.rows`, `group.items` inside a loop over groups.
fn collection_path(collection: &str) -> Option<(String, Vec<Step>)> {
    let plain = collection
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '?'));
    if !plain {
        return None;
    }
    let found = rahti_js::state_uses(Code::Expression, collection);
    match found.uses.as_slice() {
        [used] => match &used.what {
            Use::Read(steps) => Some((used.name.clone(), steps.clone())),
            Use::Other => Some((used.name.clone(), Vec::new())),
            _ => None,
        },
        _ => None,
    }
}

/// How far a value is followed through loops, `const`s and functions.
const MAX_DEPTH: usize = 24;

/// What a value is known to be.
#[derive(Clone)]
enum Held<'s> {
    /// A part of an rpc's response: its shape, the state it belongs to, the
    /// rpc whose response that state holds, and the steps from the state.
    Response(Shape, &'s str, &'s str, Vec<Step>),
    /// A part of a server value: the constant's name, which of the script's
    /// server values it is, and the steps from it. Its shape is the
    /// compiler's to know.
    Server(&'s str, usize, Vec<Step>),
}

/// A path, without where each step was written.
fn spelled(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| match step {
            Step::Field { name, .. } => format!(".{name}"),
            Step::Index => "[]".to_string(),
        })
        .collect()
}

/// What a block's names hold, once its states have their shapes.
struct Scope<'s> {
    block: &'s Block<'s>,
    /// Each state with a shape, and the rpc it holds the response of.
    typed: &'s HashMap<&'s str, (Shape, String)>,
    /// Each state given nothing but the server value it starts from, and
    /// which of the script's server values that is.
    seeded: &'s HashMap<&'s str, usize>,
    /// The script's followed `const`s and functions; `true` for a function.
    derived: &'s HashMap<&'s str, (bool, &'s Derived)>,
    shapes: &'s Shapes,
}

impl<'s> Scope<'s> {
    /// What a name holds where it is written, inside these loops: a state, a
    /// server constant, a loop's row, or one of the script's own followed
    /// `const`s.
    fn name(&self, name: &str, loops: &[String], depth: usize) -> Option<Held<'s>> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.block.meaning(name, loops) {
            Meaning::State(state) => match self.typed.get(state) {
                Some((shape, rpc)) => Some(Held::Response(shape.clone(), state, rpc, Vec::new())),
                None => {
                    let index = *self.seeded.get(state)?;
                    Some(Held::Server(state, index, Vec::new()))
                }
            },
            Meaning::Server(server, index) => Some(Held::Server(server, index, Vec::new())),
            Meaning::Row(at) => {
                let looped = parse_loop(&loops[at])?;
                let collection = rahti_js::derive(looped.collection);
                let held = self.value(&collection, &loops[..at], depth + 1)?;
                self.walk(held, looped.collection, &[Step::Index])
            }
            Meaning::Setter(_) => None,
            // The script's scope has no loop variables.
            Meaning::Nothing => match self.derived.get(name) {
                Some((false, value)) => self.value(value, &[], depth + 1),
                _ => None,
            },
        }
    }

    /// What a derived value holds, when every step of it is certain.
    fn value(&self, value: &Derived, loops: &[String], depth: usize) -> Option<Held<'s>> {
        if depth > MAX_DEPTH {
            return None;
        }
        match value {
            Derived::Name { name, steps } => {
                let held = self.name(name, loops, depth + 1)?;
                self.walk(held, name, steps)
            }
            // A function of the script, unless a loop variable or a state
            // has taken its name here.
            Derived::Call(name) => match (
                self.block.meaning(name, loops),
                self.derived.get(name.as_str()),
            ) {
                (Meaning::Nothing, Some((true, returned))) => self.value(returned, &[], depth + 1),
                _ => None,
            },
            Derived::Either(values) => {
                // Every branch that is not empty is the same thing.
                let mut found: Option<Held<'s>> = None;
                for value in values.iter().filter(|value| **value != Derived::Empty) {
                    let held = self.value(value, loops, depth + 1)?;
                    let same = match (&found, &held) {
                        (None, _) => true,
                        (
                            Some(Held::Response(shape, state, _, path)),
                            Held::Response(other, from, _, steps),
                        ) => shape == other && state == from && spelled(path) == spelled(steps),
                        (Some(Held::Server(name, _, path)), Held::Server(other, _, steps)) => {
                            name == other && spelled(path) == spelled(steps)
                        }
                        _ => false,
                    };
                    if !same {
                        return None;
                    }
                    found.get_or_insert(held);
                }
                found
            }
            Derived::SameArray(inner) => {
                let held = self.value(inner, loops, depth + 1)?;
                // Only an array answers to an array's methods. A server
                // value's shape is not known here: the method is taken at
                // its word, as it is the template's own.
                if matches!(held, Held::Response(..)) {
                    self.walk(held.clone(), "value", &[Step::Index])?;
                }
                Some(held)
            }
            Derived::Element(inner) => {
                let held = self.value(inner, loops, depth + 1)?;
                self.walk(held, "value", &[Step::Index])
            }
            Derived::Path(base, steps) => {
                let held = self.value(base, loops, depth + 1)?;
                self.walk(held, "value", steps)
            }
            Derived::Empty | Derived::Unknown => None,
        }
    }

    /// `held` after `steps`: for a response, when each one names something
    /// its shape has; for a server value, always, since the compiler decides.
    fn walk(&self, held: Held<'s>, owner: &str, steps: &[Step]) -> Option<Held<'s>> {
        if steps.is_empty() {
            return Some(held);
        }
        match held {
            Held::Response(shape, state, rpc, mut path) => {
                match self.shapes.walk(&shape, owner, steps) {
                    Walk::Known(shape) => {
                        path.extend(steps.iter().cloned());
                        Some(Held::Response(shape, state, rpc, path))
                    }
                    Walk::Open | Walk::Missing(_) => None,
                }
            }
            Held::Server(name, index, mut path) => {
                path.extend(steps.iter().cloned());
                Some(Held::Server(name, index, path))
            }
        }
    }
}

/// Every state a derived value may come from, shapes aside: what a write
/// through that value could change.
fn states_under<'s>(
    block: &Block<'s>,
    derived: &HashMap<&str, (bool, &Derived)>,
    value: &Derived,
    loops: &[String],
    depth: usize,
    out: &mut Vec<&'s str>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Derived::Name { name, .. } => match block.meaning(name, loops) {
            Meaning::State(state) => out.push(state),
            Meaning::Row(at) => {
                if let Some(looped) = parse_loop(&loops[at]) {
                    let collection = rahti_js::derive(looped.collection);
                    states_under(block, derived, &collection, &loops[..at], depth + 1, out);
                }
            }
            Meaning::Nothing => {
                if let Some((false, value)) = derived.get(name.as_str()) {
                    states_under(block, derived, value, &[], depth + 1, out);
                }
            }
            Meaning::Setter(_) | Meaning::Server(..) => {}
        },
        Derived::Call(name) => {
            if let (Meaning::Nothing, Some((true, returned))) =
                (block.meaning(name, loops), derived.get(name.as_str()))
            {
                states_under(block, derived, returned, &[], depth + 1, out);
            }
        }
        Derived::Either(values) => {
            for value in values {
                states_under(block, derived, value, loops, depth + 1, out);
            }
        }
        Derived::SameArray(inner) | Derived::Element(inner) | Derived::Path(inner, _) => {
            states_under(block, derived, inner, loops, depth + 1, out);
        }
        Derived::Empty | Derived::Unknown => {}
    }
}

/// Everything found about the block these sources were written in.
pub fn analyze(sources: &[ClientSource], shapes: &Shapes) -> Vec<Finding> {
    let mut scripts = sources
        .iter()
        .filter(|source| source.kind == ClientKind::Script && !source.foreign);
    // Markup reads one script's scope; with none, or with two, there is no
    // one set of states to speak of.
    let (Some(script), None) = (scripts.next(), scripts.next()) else {
        return Vec::new();
    };
    let declared = rahti_js::state_uses(Code::Script, &script.source);
    if declared.states.is_empty() && declared.server.is_empty() {
        return Vec::new();
    }

    let mut block = Block {
        setters: HashMap::new(),
        states: Vec::new(),
        servers: declared
            .server
            .iter()
            .map(|(name, index)| (name.as_str(), *index))
            .collect(),
    };
    // What is known against each state having a shape, and what is put in.
    let mut doubt: HashMap<&str, (&'static str, String)> = HashMap::new();
    let mut origins: HashMap<&str, Vec<&Origin>> = HashMap::new();
    for state in &declared.states {
        if block.states.contains(&state.name.as_str()) {
            doubt.insert(&state.name, ("declared twice", String::new()));
            continue;
        }
        block.states.push(&state.name);
        if let Some(setter) = &state.setter {
            block.setters.insert(setter, &state.name);
        }
        origins
            .entry(&state.name)
            .or_default()
            .extend(state.initial.iter());
    }

    // The script's own lists: what its `const`s hold and its functions
    // return, when that comes from a state without changing shape.
    let derived: HashMap<&str, (bool, &Derived)> = declared
        .derived
        .iter()
        .map(|binding| (binding.name.as_str(), (binding.function, &binding.value)))
        .collect();

    // Every use of a state or a setter, in the script and in the markup.
    struct Used<'a> {
        place: &'static str,
        source: usize,
        loops: &'a [String],
        name: &'a str,
        what: &'a Use,
    }
    let markup: Vec<(usize, &ClientSource, rahti_js::StateUses)> = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| source.kind != ClientKind::Script && !source.foreign)
        .map(|(index, source)| {
            let code = match source.kind {
                ClientKind::Handler => Code::Handler,
                _ => Code::Expression,
            };
            (index, source, rahti_js::state_uses(code, &source.source))
        })
        .collect();
    let script_index = sources
        .iter()
        .position(|source| std::ptr::eq(source, script))
        .unwrap_or_default();
    // A field some code assigns may be on any value by the time it is read.
    let all = || std::iter::once(&declared).chain(markup.iter().map(|(_, _, found)| found));
    let assigns_unnamed = all().any(|found| found.assigns_unnamed);
    let assigned: Vec<&str> = all()
        .flat_map(|found| found.assigned.iter().map(String::as_str))
        .collect();
    let mut uses: Vec<Used<'_>> = declared
        .uses
        .iter()
        .map(|used| Used {
            place: "script",
            source: script_index,
            loops: &[],
            name: &used.name,
            what: &used.what,
        })
        .collect();
    for (index, source, found) in &markup {
        let place = match source.kind {
            ClientKind::Handler => "handler",
            _ => "binding",
        };
        uses.extend(found.uses.iter().map(|used| Used {
            place,
            source: *index,
            loops: &source.loops,
            name: &used.name,
            what: &used.what,
        }));
    }

    // First, what each state is given and what speaks against a shape.
    for used in &uses {
        match (block.meaning(used.name, used.loops), used.what) {
            (Meaning::Setter(state), Use::Call(given)) => {
                origins.entry(state).or_default().extend(given.iter());
            }
            (Meaning::Setter(state), _) => {
                doubt
                    .entry(state)
                    .or_insert(("setter handed to other code", used.name.to_string()));
            }
            (Meaning::State(state), Use::Write) => {
                doubt
                    .entry(state)
                    .or_insert(("a field is written in place", String::new()));
            }
            (Meaning::Row(_), Use::Write) => {
                // A row written to changes what its state holds: every state
                // the row may come from.
                let row = Derived::Name {
                    name: used.name.to_string(),
                    steps: Vec::new(),
                };
                let mut states = Vec::new();
                states_under(&block, &derived, &row, used.loops, 0, &mut states);
                for state in states {
                    doubt
                        .entry(state)
                        .or_insert(("a row's field is written in place", String::new()));
                }
            }
            _ => {}
        }
    }

    // Then each state's shape: the one response everything given agrees on.
    let mut typed: HashMap<&str, (Shape, String)> = HashMap::new();
    // The server value each seeded state starts from: one that is also
    // given a response, and one that is given nothing else.
    let mut seeds: HashMap<&str, usize> = HashMap::new();
    let mut seeded: HashMap<&str, usize> = HashMap::new();
    let mut findings = Vec::new();
    for name in &block.states {
        let shape = match doubt.remove(name) {
            Some(reason) => Err(reason),
            None => join(origins.get(name).map_or(&[][..], Vec::as_slice), shapes),
        };
        let shape = match shape {
            Ok(Joined::Response(shape, rpc, seed)) => {
                let written = shape.to_string();
                typed.insert(name, (shape, rpc));
                if let Some(seed) = seed {
                    seeds.insert(name, seed);
                }
                Ok(written)
            }
            Ok(Joined::Seed(index)) => {
                seeded.insert(name, index);
                Ok("the server value it starts from".to_string())
            }
            Err(reason) => Err(reason),
        };
        findings.push(Finding::State {
            name: name.to_string(),
            shape,
        });
    }

    let scope = Scope {
        block: &block,
        typed: &typed,
        seeded: &seeded,
        derived: &derived,
        shapes,
    };

    // Loops, once each.
    let mut seen: Vec<&[String]> = Vec::new();
    for (_, source, _) in &markup {
        for depth in 0..source.loops.len() {
            let loops = &source.loops[..=depth];
            if seen.contains(&loops) {
                continue;
            }
            seen.push(loops);
            if let Some(looped) = parse_loop(&loops[depth]) {
                let of = match collection_path(looped.collection) {
                    None if rahti_js::derive(looped.collection) != Derived::Unknown => {
                        "a derived list".to_string()
                    }
                    None => "an expression".to_string(),
                    Some((root, _)) => match block.meaning(&root, &loops[..depth]) {
                        Meaning::State(state) => format!("state {state}"),
                        Meaning::Row(_) => "an outer row".to_string(),
                        Meaning::Server(..) => "a server value".to_string(),
                        Meaning::Setter(_) | Meaning::Nothing => "not a state".to_string(),
                    },
                };
                findings.push(Finding::Loop {
                    directive: loops[depth].clone(),
                    typed: scope.name(looped.item, loops, 0).is_some(),
                    of,
                });
            }
        }
    }

    // Last, each field read off a value that has a shape.
    for used in &uses {
        let Use::Read(steps) = used.what else {
            continue;
        };
        let place = match block.meaning(used.name, used.loops) {
            Meaning::State(_) | Meaning::Server(..) => used.place,
            Meaning::Row(_) => "row",
            _ => continue,
        };
        let mut expression = used.name.to_string();
        expression.push_str(&spelled(steps).replace(".[]", "[]"));
        match scope.name(used.name, used.loops, 0) {
            None => {}
            Some(Held::Response(shape, state, rpc, path)) => {
                let outcome = match shapes.walk(&shape, used.name, steps) {
                    Walk::Known(_) => Outcome::Known,
                    Walk::Open => Outcome::Open,
                    Walk::Missing(missing) => match steps.get(missing.step) {
                        Some(Step::Field { name, span })
                            if !assigns_unnamed && !assigned.contains(&name.as_str()) =>
                        {
                            match seeds.get(state) {
                                None => Outcome::Missing {
                                    owner: missing.owner,
                                    fields: missing.fields,
                                    field: name.clone(),
                                    span: span.clone(),
                                    state: state.to_string(),
                                    rpc: rpc.to_string(),
                                },
                                // The response lacks the field; the server
                                // value the state starts from may have it.
                                // Whether it does is the compiler's to say.
                                Some(index) => {
                                    findings.push(Finding::ServerRead {
                                        place,
                                        source: used.source,
                                        root: used.name.to_string(),
                                        expression: expression.clone(),
                                        from: From::SeededState(state.to_string(), rpc.to_string()),
                                        index: *index,
                                        before: path.len(),
                                        path: path
                                            .iter()
                                            .chain(&steps[..=missing.step])
                                            .cloned()
                                            .collect(),
                                        last_only: true,
                                    });
                                    Outcome::Open
                                }
                            }
                        }
                        // A field this block gives values in place may be there.
                        _ => Outcome::Open,
                    },
                };
                findings.push(Finding::Read {
                    place,
                    source: used.source,
                    expression,
                    outcome,
                });
            }
            Some(Held::Server(name, index, path)) => {
                // The read as far as it names fields nothing in this block
                // could have added: the compiler is asked about those.
                let certain = steps
                    .iter()
                    .position(|step| match step {
                        Step::Field { name, .. } => {
                            assigned.contains(&name.as_str())
                                || crate::rpc_shapes::TypeLevel::object_member(name)
                        }
                        Step::Index => false,
                    })
                    .unwrap_or(steps.len());
                let steps = &steps[..certain];
                if assigns_unnamed || !steps.iter().any(|step| matches!(step, Step::Field { .. })) {
                    continue;
                }
                let from = if block.states.contains(&name) {
                    From::State(name.to_string())
                } else {
                    From::Constant(name.to_string())
                };
                findings.push(Finding::ServerRead {
                    place,
                    source: used.source,
                    root: used.name.to_string(),
                    expression,
                    from,
                    index,
                    before: path.len(),
                    path: path.into_iter().chain(steps.iter().cloned()).collect(),
                    last_only: false,
                });
            }
        }
    }
    findings
}

/// What a state holds.
enum Joined {
    /// One response's shape, the rpc it is the response of, and the server
    /// value the state starts from, when it starts from one.
    Response(Shape, String, Option<usize>),
    /// Nothing but the server value it starts from.
    Seed(usize),
}

/// What everything put into a state agrees it holds, or why that is not
/// known.
fn join(origins: &[&Origin], shapes: &Shapes) -> Result<Joined, (&'static str, String)> {
    let mut found: Option<(Shape, String)> = None;
    let mut seed: Option<usize> = None;
    let mut other: Vec<&str> = Vec::new();
    for origin in origins {
        match origin {
            Origin::Empty => {}
            Origin::Server(Some(index)) if seed.is_none() => seed = Some(*index),
            // A second server value, or one read some other way.
            Origin::Server(_) => other.push("@{…}"),
            Origin::Unknown(written) => other.push(written),
            Origin::Rpc { rpc, steps } => {
                let Some(response) = shapes.response(rpc) else {
                    return Err(("response shape not known", rpc.clone()));
                };
                let Walk::Known(shape) = shapes.walk(&response, rpc, steps) else {
                    return Err(("response part not followed", rpc.clone()));
                };
                match &found {
                    None => found = Some((shape, rpc.clone())),
                    Some((first, _)) if *first == shape => {}
                    Some((first, from)) => {
                        return Err((
                            "given two different shapes",
                            format!("{first} from {from}, {shape} from {rpc}"),
                        ));
                    }
                }
            }
        }
    }
    other.dedup();
    match (found, seed, other.first()) {
        (Some((shape, rpc)), seed, None) => Ok(Joined::Response(shape, rpc, seed)),
        (None, Some(seed), None) => Ok(Joined::Seed(seed)),
        (Some(_), _, Some(written)) => Err(("given something else", written.to_string())),
        (None, _, _) => Err(("never given an rpc result", other.join(" ‖ "))),
    }
}

#[cfg(test)]
#[path = "tests/state_types.rs"]
mod tests;
