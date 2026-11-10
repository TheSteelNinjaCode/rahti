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
}

impl Shapes {
    /// The shapes `file` owns, read from the text the build writes: one
    /// `R\tfile\trpc\ttype` line per response and one `T\tfile\thead\tbody`
    /// line per declaration.
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
    /// any. Read again whenever the file on disk changes, because an editor
    /// keeps one macro process across many builds.
    pub fn current() -> Option<Arc<Shapes>> {
        /// The shapes file, the source file, when it was written, and what it held.
        type Cached = (PathBuf, String, SystemTime, Arc<Shapes>);
        static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

        if !proc_macro::is_available() {
            return None;
        }
        let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
        let file = relative(&root, &proc_macro::Span::call_site().local_file()?)?;
        let path = root.join(SHAPES_PATH);
        let modified = std::fs::metadata(&path).ok()?.modified().ok()?;

        let mut cache = CACHE.lock().ok()?;
        if let Some((cached_path, cached_file, at, shapes)) = cache.as_ref()
            && *cached_path == path
            && *cached_file == file
            && *at == modified
        {
            return Some(shapes.clone());
        }
        let shapes = Arc::new(Shapes::parse(&std::fs::read_to_string(&path).ok()?, &file));
        *cache = Some((path, file, modified, shapes.clone()));
        Some(shapes)
    }

    /// The first step of `steps`, read off `rpc`'s response, that names a
    /// field the response does not have.
    pub fn missing(&self, rpc: &str, steps: &[Step]) -> Option<Missing> {
        let response = self.responses.get(rpc)?;
        let mut ty = parse(response)?;
        let mut owner = response.clone();
        for (index, step) in steps.iter().enumerate() {
            let next = match step {
                Step::Index => self.element(&ty, 0),
                Step::Field { name, .. } => match self.field(&ty, name, 0) {
                    Outcome::Type(ty) => Some(ty),
                    Outcome::Open => None,
                    Outcome::Missing(mut fields) => {
                        fields.sort();
                        fields.dedup();
                        return Some(Missing {
                            step: index,
                            owner,
                            fields,
                        });
                    }
                },
            };
            let next = next?;
            if let Step::Field { name, .. } = step {
                owner = format!("{owner}.{name}");
            }
            ty = next;
        }
        None
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

enum Outcome {
    Type(Ty),
    /// Nothing certain can be said about the field.
    Open,
    Missing(Vec<String>),
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
