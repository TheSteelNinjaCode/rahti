//! Checking what client code reads off a `pp.rpc` result against the shape
//! the `#[rpc]` returns.
//!
//! The build writes every rpc's response shape to `.rahti/rpc-shapes.tsv`
//! before this crate's macros run; `rahti-js` finds the reads; this module
//! walks each read through the shape and refuses a field the response does
//! not have.
//!
//! It refuses only what it is sure of. A step is an error when the value is
//! an object whose every field is known and the name is not one of them.
//! Anything else — an array's methods, a string's, `unknown`, a `Record`, a
//! type the build could not follow, a missing shapes file — ends the walk
//! without a word, because a wrong refusal stops a correct application from
//! compiling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rahti_js::Step;

/// Where the build leaves the shapes, below the crate being compiled.
const SHAPES_PATH: &str = ".rahti/rpc-shapes.tsv";

/// The shapes of one source file's rpcs.
#[derive(Default, Debug)]
pub struct Shapes {
    /// Rpc name to its response type.
    responses: HashMap<String, String>,
    /// Type name to its parameters and body.
    types: HashMap<String, (Vec<String>, String)>,
    /// Under the file `*`: every struct of the crate a generated module can
    /// name, as `(path, type)`, in the order the build wrote them.
    named: Vec<(String, String)>,
    /// Every field name of every object these shapes declare.
    fields: std::sync::OnceLock<std::collections::BTreeSet<String>>,
    /// Each component the file can call, and the props it reads: `None`
    /// when it may read any.
    components: HashMap<String, Option<std::collections::BTreeSet<String>>>,
}

/// The file the crate-wide shapes are written under.
pub const CRATE_WIDE: &str = "*";

impl Shapes {
    /// The shapes `file` owns, read from the text the build writes: one
    /// `R\tfile\trpc\ttype` line per response and one `T\tfile\thead\tbody`
    /// line per declaration, and, under the file `*`, one `G\t*\tpath\ttype`
    /// line per struct of the crate a generated module can name.
    pub fn parse(text: &str, file: &str) -> Shapes {
        let mut shapes = Shapes::default();
        for line in text.lines() {
            let mut parts = line.splitn(4, '\t');
            let (Some(kind), Some(owner), Some(name), Some(body)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if owner != file {
                continue;
            }
            match kind {
                "R" => {
                    shapes.responses.insert(name.to_string(), body.to_string());
                }
                "G" => shapes.named.push((name.to_string(), body.to_string())),
                "C" => {
                    let props =
                        (body != "*").then(|| body.split_whitespace().map(String::from).collect());
                    shapes.components.insert(name.to_string(), props);
                }
                "T" => {
                    let (name, params) = match name.split_once('<') {
                        Some((name, params)) => (
                            name,
                            params
                                .trim_end_matches('>')
                                .split(',')
                                .map(|param| param.trim().to_string())
                                .collect(),
                        ),
                        None => (name, Vec::new()),
                    };
                    shapes
                        .types
                        .insert(name.to_string(), (params, body.to_string()));
                }
                _ => {}
            }
        }
        shapes
    }

    /// The shapes of the file the macro is expanding in, when the build wrote
    /// any.
    pub fn current() -> Option<Arc<Shapes>> {
        Self::load(&current_file()?)
    }

    /// The shapes of every struct of the crate a generated module can name.
    pub fn crate_wide() -> Option<Arc<Shapes>> {
        Self::load(CRATE_WIDE)
    }

    /// The shapes written under `file`. Read again whenever the shapes file
    /// on disk changes, because an editor keeps one macro process across
    /// many builds.
    fn load(file: &str) -> Option<Arc<Shapes>> {
        /// The shapes file, when it was written, its text, and what has been
        /// read from it per file.
        type Cached = (PathBuf, SystemTime, String, HashMap<String, Arc<Shapes>>);
        static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

        if !proc_macro::is_available() {
            return None;
        }
        let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
        let path = root.join(SHAPES_PATH);
        let modified = std::fs::metadata(&path).ok()?.modified().ok()?;

        let mut cache = CACHE.lock().ok()?;
        let fresh =
            matches!(cache.as_ref(), Some((cached, at, ..)) if *cached == path && *at == modified);
        if !fresh {
            let text = std::fs::read_to_string(&path).ok()?;
            *cache = Some((path, modified, text, HashMap::new()));
        }
        let (_, _, text, read) = cache.as_mut()?;
        if let Some(shapes) = read.get(file) {
            return Some(shapes.clone());
        }
        let shapes = Arc::new(Shapes::parse(text, file));
        read.insert(file.to_string(), shapes.clone());
        Some(shapes)
    }

    /// The props the component a tag names reads, when the build found the
    /// component and every way it reads a prop.
    pub fn props_read(&self, component: &str) -> Option<&std::collections::BTreeSet<String>> {
        self.components.get(component)?.as_ref()
    }

    /// Whether any struct has a shape here.
    pub fn has_structs(&self) -> bool {
        !self.named.is_empty()
    }

    /// Every field name of every object these shapes declare: the names a
    /// field trait exists for in the generated module.
    pub fn fields(&self) -> &std::collections::BTreeSet<String> {
        fn collect(ty: &Ty, out: &mut std::collections::BTreeSet<String>) {
            match ty {
                Ty::Object(fields) => {
                    for (name, ty) in fields {
                        out.insert(name.clone());
                        collect(ty, out);
                    }
                }
                Ty::Array(element) => collect(element, out),
                Ty::Union(members) | Ty::Intersection(members) => {
                    members.iter().for_each(|member| collect(member, out));
                }
                Ty::Named(_, args) => args.iter().for_each(|arg| collect(arg, out)),
                Ty::Tuple | Ty::Other => {}
            }
        }
        self.fields.get_or_init(|| {
            let mut out = std::collections::BTreeSet::new();
            for (_, body) in self.types.values() {
                if let Some(ty) = parse(body) {
                    collect(&ty, &mut out);
                }
            }
            out
        })
    }

    /// The name of the trait the generated module declares for a field.
    /// Every character that cannot stand in an identifier is spelled out, so
    /// two fields never share one.
    pub fn field_trait(field: &str) -> proc_macro2::Ident {
        let mut name = String::from("F_");
        for c in field.chars() {
            if c.is_ascii_alphanumeric() {
                name.push(c);
            } else {
                name.push_str(&format!("_{:x}_", c as u32));
            }
        }
        proc_macro2::Ident::new(&name, proc_macro2::Span::call_site())
    }

    /// The first step of `steps`, read off `rpc`'s response, that names a
    /// field the response does not have.
    pub fn missing(&self, rpc: &str, steps: &[Step]) -> Option<Missing> {
        let owner = self.responses.get(rpc)?;
        match self.walk(&self.response(rpc)?, owner, steps) {
            Walk::Missing(missing) => Some(missing),
            Walk::Known(_) | Walk::Open => None,
        }
    }

    /// The shape of `rpc`'s response, when the build wrote one this reads.
    pub fn response(&self, rpc: &str) -> Option<Shape> {
        parse(self.responses.get(rpc)?).map(Shape)
    }

    /// Follows `steps` from a value of shape `from`, which a message names
    /// `owner`.
    pub fn walk(&self, from: &Shape, owner: &str, steps: &[Step]) -> Walk {
        let mut ty = from.0.clone();
        let mut owner = owner.to_string();
        for (index, step) in steps.iter().enumerate() {
            let next = match step {
                Step::Index => self.element(&ty, 0),
                Step::Field { name, .. } => match self.field(&ty, name, 0) {
                    Outcome::Type(ty) => Some(ty),
                    Outcome::Open => None,
                    Outcome::Missing(mut fields) => {
                        fields.sort();
                        fields.dedup();
                        return Walk::Missing(Missing {
                            step: index,
                            owner,
                            fields,
                        });
                    }
                },
            };
            let Some(next) = next else {
                return Walk::Open;
            };
            if let Step::Field { name, .. } = step {
                owner = format!("{owner}.{name}");
            }
            ty = next;
        }
        Walk::Known(Shape(ty))
    }

    /// The crate's structs as types the compiler can follow a read through
    /// (see `rahti::shape`): a marker type per object, whose name spells the
    /// object's fields, since an error can show a type and nothing else.
    pub fn type_level(&self) -> TypeLevel {
        let mut out = TypeLevel::default();
        let mut memo = HashMap::new();
        for (path, written) in &self.named {
            // A struct with named fields: nothing else has fields to read.
            let Some(Ty::Named(name, args)) = parse(written) else {
                continue;
            };
            if !args.is_empty() || !matches!(self.expand(&name, &[]), Some(Ty::Object(_))) {
                continue;
            }
            let Ok(path) = path.parse::<proc_macro2::TokenStream>() else {
                continue;
            };
            let hint = name.clone();
            let node = self.node(&Ty::Named(name, Vec::new()), hint, 0, &mut memo, &mut out);
            out.named.push((path, node));
        }
        out
    }

    /// `ty` as a type-level shape, declaring a marker for each object in it.
    fn node(
        &self,
        ty: &Ty,
        hint: String,
        depth: usize,
        memo: &mut HashMap<String, proc_macro2::Ident>,
        out: &mut TypeLevel,
    ) -> proc_macro2::TokenStream {
        use quote::quote;
        let open = quote! { ::rahti::shape::Open };
        if depth > MAX_DEPTH {
            return open;
        }
        match ty {
            Ty::Named(name, args) if args.is_empty() => {
                if let Some(marker) = memo.get(name) {
                    return quote! { #marker };
                }
                match self.expand(name, &[]) {
                    Some(Ty::Object(fields)) => {
                        // Declared before its fields are read, so a struct
                        // that contains itself names its own marker.
                        let marker = out.marker(name, &fields);
                        memo.insert(name.clone(), marker.clone());
                        self.object(&marker, name, &fields, depth, memo, out);
                        quote! { #marker }
                    }
                    Some(other) => self.node(&other, hint, depth + 1, memo, out),
                    None => open,
                }
            }
            Ty::Named(name, args) => match self.expand(name, args) {
                Some(ty) => self.node(&ty, hint, depth + 1, memo, out),
                None => open,
            },
            Ty::Object(fields) => {
                let marker = out.marker(&hint, fields);
                self.object(&marker, &hint, fields, depth, memo, out);
                quote! { #marker }
            }
            Ty::Array(element) => {
                let element = self.node(element, hint, depth + 1, memo, out);
                quote! { ::rahti::shape::Arr<#element> }
            }
            Ty::Union(members) => {
                // `T | null` is `T` or nothing; any other choice is open.
                let mut present = members.iter().filter(|member| !member.is_null());
                match (present.next(), present.next()) {
                    (Some(only), None) => {
                        let inner = self.node(only, hint, depth + 1, memo, out);
                        if members.iter().any(Ty::is_null) {
                            quote! { ::rahti::shape::Maybe<#inner> }
                        } else {
                            inner
                        }
                    }
                    _ => open,
                }
            }
            Ty::Intersection(_) | Ty::Tuple | Ty::Other => open,
        }
    }

    fn object(
        &self,
        marker: &proc_macro2::Ident,
        hint: &str,
        fields: &[(String, Ty)],
        depth: usize,
        memo: &mut HashMap<String, proc_macro2::Ident>,
        out: &mut TypeLevel,
    ) {
        let fields = fields
            .iter()
            .map(|(name, ty)| {
                let node = self.node(ty, format!("{hint}_{name}"), depth + 1, memo, out);
                (name.clone(), node)
            })
            .collect();
        out.objects.push((marker.clone(), fields));
    }

    /// A named type's body, with its parameters replaced by `args`.
    fn expand(&self, name: &str, args: &[Ty]) -> Option<Ty> {
        let (params, body) = self.types.get(name)?;
        let mut ty = parse(body)?;
        if !params.is_empty() {
            let bound: HashMap<&str, &Ty> =
                params.iter().map(String::as_str).zip(args.iter()).collect();
            ty = substitute(ty, &bound);
        }
        Some(ty)
    }

    fn field(&self, ty: &Ty, name: &str, depth: usize) -> Outcome {
        if depth > MAX_DEPTH {
            return Outcome::Open;
        }
        match ty {
            Ty::Object(fields) => match fields.iter().find(|(field, _)| field == name) {
                Some((_, ty)) => Outcome::Type(ty.clone()),
                // What every JavaScript object answers to, whatever its shape.
                None if OBJECT_MEMBERS.contains(&name) => Outcome::Open,
                None => Outcome::Missing(fields.iter().map(|(field, _)| field.clone()).collect()),
            },
            Ty::Named(named, args) => match self.expand(named, args) {
                Some(ty) => self.field(&ty, name, depth + 1),
                None => Outcome::Open,
            },
            Ty::Union(members) => {
                // `T | null`: the read is of `T`. Among several shapes, one
                // that has the field is enough.
                let mut found = Vec::new();
                let mut missing = Vec::new();
                for member in members.iter().filter(|member| !member.is_null()) {
                    match self.field(member, name, depth + 1) {
                        Outcome::Open => return Outcome::Open,
                        Outcome::Type(ty) => found.push(ty),
                        Outcome::Missing(fields) => missing.extend(fields),
                    }
                }
                match found.len() {
                    0 if missing.is_empty() => Outcome::Open,
                    0 => Outcome::Missing(missing),
                    1 => Outcome::Type(found.remove(0)),
                    _ => Outcome::Type(Ty::Union(found)),
                }
            }
            Ty::Intersection(members) => {
                let mut missing = Vec::new();
                let mut found = None;
                for member in members {
                    match self.field(member, name, depth + 1) {
                        Outcome::Open => return Outcome::Open,
                        Outcome::Type(ty) => found = found.or(Some(ty)),
                        Outcome::Missing(fields) => missing.extend(fields),
                    }
                }
                match found {
                    Some(ty) => Outcome::Type(ty),
                    None => Outcome::Missing(missing),
                }
            }
            Ty::Array(_) | Ty::Tuple | Ty::Other => Outcome::Open,
        }
    }

    fn element(&self, ty: &Ty, depth: usize) -> Option<Ty> {
        if depth > MAX_DEPTH {
            return None;
        }
        match ty {
            Ty::Array(element) => Some((**element).clone()),
            Ty::Named(named, args) => self.element(&self.expand(named, args)?, depth + 1),
            Ty::Union(members) => {
                let mut present = members.iter().filter(|member| !member.is_null());
                let only = present.next()?;
                match present.next() {
                    None => self.element(only, depth + 1),
                    Some(_) => None,
                }
            }
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 32;

/// Members of `Object.prototype`, readable on any object.
const OBJECT_MEMBERS: &[&str] = &[
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
];

/// How long a marker's name grows before the rest of its fields are counted
/// rather than spelled.
const MARKER_LENGTH: usize = 32;

/// The crate's structs, as type-level shapes.
#[derive(Default)]
pub struct TypeLevel {
    /// Each struct's path, and its shape.
    pub named: Vec<(proc_macro2::TokenStream, proc_macro2::TokenStream)>,
    /// Each object's marker, and its fields with their shapes.
    pub objects: Vec<(proc_macro2::Ident, Vec<(String, proc_macro2::TokenStream)>)>,
    taken: std::collections::HashSet<String>,
}

impl TypeLevel {
    /// A new marker for an object: `Project__id__name__owner`, made unique.
    fn marker(&mut self, hint: &str, fields: &[(String, Ty)]) -> proc_macro2::Ident {
        let word = |text: &str| -> String {
            text.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect()
        };
        let mut name = word(hint);
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            name.insert(0, 'T');
        }
        // The compiler leaves a long type name out of its message, so a
        // struct with many fields names the first of them and counts the rest.
        for (index, (field, _)) in fields.iter().enumerate() {
            if name.len() > MARKER_LENGTH {
                name.push_str(&format!("__and_{}_more", fields.len() - index));
                break;
            }
            name.push_str("__");
            name.push_str(&word(field));
        }
        let mut unique = name.clone();
        let mut count = 1;
        while !self.taken.insert(unique.clone()) {
            count += 1;
            unique = format!("{name}__{count}");
        }
        proc_macro2::Ident::new(&unique, proc_macro2::Span::call_site())
    }

    /// The members of `Object.prototype`, which every object answers to.
    pub fn object_member(name: &str) -> bool {
        OBJECT_MEMBERS.contains(&name)
    }
}

/// A step that names no field of the value it reads.
#[derive(Debug, PartialEq, Eq)]
pub struct Missing {
    /// Which step of the read.
    pub step: usize,
    /// The value read from, as the caller would write it: `Saved.owner`.
    pub owner: String,
    /// The fields that value does have.
    pub fields: Vec<String>,
}

/// The shape of a value, as far as a field read needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape(Ty);

impl std::fmt::Display for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Where following a read through a shape ends.
#[derive(Debug, PartialEq)]
pub enum Walk {
    /// Every step named something the shape has: the value read.
    Known(Shape),
    /// Nothing certain can be said: an array's or a string's member,
    /// `unknown`, a type the build could not follow.
    Open,
    /// A step names a field the value does not have.
    Missing(Missing),
}

enum Outcome {
    Type(Ty),
    /// Nothing certain can be said about the field.
    Open,
    Missing(Vec<String>),
}

/// The file the macro is expanding in, as the build names it.
pub fn current_file() -> Option<String> {
    if !proc_macro::is_available() {
        return None;
    }
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    relative(&root, &proc_macro::Span::call_site().local_file()?)
}

/// `path` below `root`, with forward slashes: how the build names a file.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let path = path.strip_prefix(root).unwrap_or(path);
    if path.is_absolute() {
        return None;
    }
    Some(path.to_string_lossy().replace('\\', "/"))
}

// ----- the shapes the build writes ---------------------------------------

/// A shape, as far as a field read needs it.
#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Object(Vec<(String, Ty)>),
    Named(String, Vec<Ty>),
    Array(Box<Ty>),
    Union(Vec<Ty>),
    Intersection(Vec<Ty>),
    Tuple,
    /// A primitive, a literal, `unknown`: nothing with fields to check.
    Other,
}

impl Ty {
    fn is_null(&self) -> bool {
        matches!(self, Ty::Named(name, args) if name == "null" && args.is_empty())
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let join = |f: &mut std::fmt::Formatter<'_>, members: &[Ty], with: &str| {
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    f.write_str(with)?;
                }
                member.fmt(f)?;
            }
            Ok(())
        };
        match self {
            Ty::Object(fields) => {
                f.write_str("{ ")?;
                for (index, (name, _)) in fields.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    f.write_str(name)?;
                }
                f.write_str(" }")
            }
            Ty::Named(name, args) if args.is_empty() => f.write_str(name),
            Ty::Named(name, args) => {
                write!(f, "{name}<")?;
                join(f, args, ", ")?;
                f.write_str(">")
            }
            Ty::Array(element) => write!(f, "{element}[]"),
            Ty::Union(members) => join(f, members, " | "),
            Ty::Intersection(members) => join(f, members, " & "),
            Ty::Tuple => f.write_str("[…]"),
            Ty::Other => f.write_str("…"),
        }
    }
}

fn substitute(ty: Ty, bound: &HashMap<&str, &Ty>) -> Ty {
    match ty {
        Ty::Named(name, args) if args.is_empty() => match bound.get(name.as_str()) {
            Some(ty) => (*ty).clone(),
            None => Ty::Named(name, args),
        },
        Ty::Named(name, args) => Ty::Named(
            name,
            args.into_iter().map(|arg| substitute(arg, bound)).collect(),
        ),
        Ty::Object(fields) => Ty::Object(
            fields
                .into_iter()
                .map(|(name, ty)| (name, substitute(ty, bound)))
                .collect(),
        ),
        Ty::Array(element) => Ty::Array(Box::new(substitute(*element, bound))),
        Ty::Union(members) => {
            Ty::Union(members.into_iter().map(|m| substitute(m, bound)).collect())
        }
        Ty::Intersection(members) => {
            Ty::Intersection(members.into_iter().map(|m| substitute(m, bound)).collect())
        }
        other => other,
    }
}

/// Reads a type as `rahti-build` writes one. `None` for anything else: a
/// shape this cannot read is a shape it does not check against.
fn parse(text: &str) -> Option<Ty> {
    let mut parser = Parser {
        text: text.as_bytes(),
        at: 0,
    };
    let ty = parser.union()?;
    parser.space();
    (parser.at == parser.text.len()).then_some(ty)
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.text.get(self.at) == Some(&b' ') {
            self.at += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.space();
        if self.text[self.at..].starts_with(token.as_bytes()) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    fn union(&mut self) -> Option<Ty> {
        let mut members = vec![self.intersection()?];
        while self.eat("|") {
            members.push(self.intersection()?);
        }
        Some(if members.len() == 1 {
            members.remove(0)
        } else {
            Ty::Union(members)
        })
    }

    fn intersection(&mut self) -> Option<Ty> {
        let mut members = vec![self.postfix()?];
        while self.eat("&") {
            members.push(self.postfix()?);
        }
        Some(if members.len() == 1 {
            members.remove(0)
        } else {
            Ty::Intersection(members)
        })
    }

    fn postfix(&mut self) -> Option<Ty> {
        let mut ty = self.primary()?;
        while self.eat("[]") {
            ty = Ty::Array(Box::new(ty));
        }
        Some(ty)
    }

    fn primary(&mut self) -> Option<Ty> {
        self.space();
        match *self.text.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                loop {
                    if self.eat("}") {
                        break;
                    }
                    let name = self.key()?;
                    self.eat("?");
                    if !self.eat(":") {
                        return None;
                    }
                    fields.push((name, self.union()?));
                    if !self.eat(";") && !self.text[self.at..].trim_ascii_start().starts_with(b"}")
                    {
                        return None;
                    }
                }
                Some(Ty::Object(fields))
            }
            b'[' => {
                self.at += 1;
                if !self.eat("]") {
                    loop {
                        self.union()?;
                        if self.eat("]") {
                            break;
                        }
                        if !self.eat(",") {
                            return None;
                        }
                    }
                }
                Some(Ty::Tuple)
            }
            b'(' => {
                self.at += 1;
                let ty = self.union()?;
                self.eat(")").then_some(ty)
            }
            b'"' => {
                self.string()?;
                Some(Ty::Other)
            }
            _ => {
                let name = self.word()?;
                let mut args = Vec::new();
                if self.eat("<") {
                    loop {
                        args.push(self.union()?);
                        if self.eat(">") {
                            break;
                        }
                        if !self.eat(",") {
                            return None;
                        }
                    }
                }
                Some(match (name.as_str(), args.len()) {
                    ("Array", 1) => Ty::Array(Box::new(args.remove(0))),
                    (
                        "string" | "number" | "boolean" | "unknown" | "never" | "File" | "Record",
                        _,
                    ) => Ty::Other,
                    _ => Ty::Named(name, args),
                })
            }
        }
    }

    fn word(&mut self) -> Option<String> {
        self.space();
        let start = self.at;
        while self
            .text
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
        {
            self.at += 1;
        }
        (self.at > start).then(|| String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
    }

    /// A property name: an identifier, or a JSON string.
    fn key(&mut self) -> Option<String> {
        self.space();
        if self.text.get(self.at) == Some(&b'"') {
            self.string()
        } else {
            self.word()
        }
    }

    /// A JSON string, with its common escapes read.
    fn string(&mut self) -> Option<String> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            match *self.text.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return String::from_utf8(out).ok();
                }
                b'\\' => {
                    out.push(match *self.text.get(self.at + 1)? {
                        b'n' => b'\n',
                        b't' => b'\t',
                        // `\uXXXX` and the rest: a name nobody reads with a dot.
                        b'u' => return None,
                        other => other,
                    });
                    self.at += 2;
                }
                other => {
                    out.push(other);
                    self.at += 1;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/rpc_shapes.rs"]
mod tests;
