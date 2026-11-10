//! TypeScript shapes for the JSON an rpc receives and returns.
//!
//! A `#[rpc]` signature spells Rust types; the browser sends and reads JSON.
//! This module follows each spelling to its definition — through the file's
//! own items, its `use` declarations and the crate's module files — and
//! writes what Serde makes of it as a TypeScript type, container and field
//! attributes included.
//!
//! It reads source, not types, so it knows only what the source says. A type
//! from another crate, a hand-written `Serialize`, or a `with = "…"` field is
//! `unknown`, and the shape that contains it is reported as partial rather
//! than guessed at: a shape is a description to read, and a wrong one is
//! worse than a missing one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use syn::{Attribute, Fields, GenericArgument, Item, PathArguments, Type, UseTree};

/// Which side of the wire a type is on, which decides the Serde attributes
/// that apply to it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Direction {
    /// A parameter: deserialized from what the browser sent.
    Receive,
    /// A return value: serialized for the browser to read.
    Send,
}

/// How much of a shape the source accounted for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Every part was followed to a definition or a known type.
    Resolved,
    /// Some part is `unknown` because its definition could not be read.
    Partial,
    /// Nothing about the type could be read.
    Unresolved,
    /// The type is JSON of any shape, such as `serde_json::Value`.
    Dynamic,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Resolved => "resolved",
            Status::Partial => "partial",
            Status::Unresolved => "unresolved",
            Status::Dynamic => "dynamic",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shape {
    pub typescript: String,
    pub status: Status,
}

/// The source files read so far, parsed once each.
#[derive(Default)]
pub struct Sources {
    files: HashMap<PathBuf, Option<Rc<syn::File>>>,
    /// Every file a shape was read from, so the build reruns when one changes.
    pub read: BTreeSet<PathBuf>,
}

impl Sources {
    fn file(&mut self, path: &Path) -> Option<Rc<syn::File>> {
        if let Some(parsed) = self.files.get(path) {
            return parsed.clone();
        }
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|source| syn::parse_file(&source).ok())
            .map(Rc::new);
        if parsed.is_some() {
            self.read.insert(path.to_path_buf());
        }
        self.files.insert(path.to_path_buf(), parsed.clone());
        parsed
    }
}

/// A module: a file, and the inline `mod` blocks entered inside it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Module {
    file: PathBuf,
    mods: Vec<String>,
}

enum Located {
    Item(Module, Box<Item>),
    /// Not in this crate's source, as far as could be followed.
    External(Vec<String>),
}

/// A text and whether every part of it was accounted for.
type Ts = (String, bool);

fn unknown() -> Ts {
    ("unknown".to_string(), false)
}

/// The shapes one file's rpcs use. Named types are declared once per file
/// and referred to by name, so a type that contains itself is still finite.
pub struct Scope<'a> {
    sources: &'a mut Sources,
    home: Module,
    root: Option<Module>,
    names: HashMap<(Module, String, Direction), String>,
    complete: HashMap<String, bool>,
    arity: HashMap<String, usize>,
    /// Declaration head (`Page<Row>`) to its body.
    pub types: BTreeMap<String, String>,
    /// A name is being followed to a function rather than to a type.
    functions: bool,
}

const MAX_DEPTH: usize = 16;

impl<'a> Scope<'a> {
    pub fn new(sources: &'a mut Sources, file: &str) -> Self {
        let file = PathBuf::from(file);
        let root = file.ancestors().skip(1).take(MAX_DEPTH).find_map(|dir| {
            ["lib.rs", "main.rs"]
                .iter()
                .map(|name| dir.join(name))
                .find(|path| path.is_file())
        });
        Scope {
            sources,
            home: Module {
                file,
                mods: Vec::new(),
            },
            root: root.map(|file| Module {
                file,
                mods: Vec::new(),
            }),
            names: HashMap::new(),
            complete: HashMap::new(),
            arity: HashMap::new(),
            types: BTreeMap::new(),
            functions: false,
        }
    }

    /// The shape of a type written in this scope's file.
    pub fn shape(&mut self, ty: &Type, direction: Direction) -> Shape {
        let home = self.home.clone();
        let (typescript, complete) = self.of(&home, ty, direction, &[], 0);
        let status = match (typescript == "unknown", complete) {
            (true, true) => Status::Dynamic,
            (true, false) => Status::Unresolved,
            (false, true) => Status::Resolved,
            (false, false) => Status::Partial,
        };
        Shape { typescript, status }
    }

    fn of(
        &mut self,
        at: &Module,
        ty: &Type,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        if depth > MAX_DEPTH {
            return unknown();
        }
        match ty {
            Type::Reference(reference) => self.of(at, &reference.elem, direction, generics, depth),
            Type::Paren(paren) => self.of(at, &paren.elem, direction, generics, depth),
            Type::Group(group) => self.of(at, &group.elem, direction, generics, depth),
            Type::Array(array) => array_of(self.of(at, &array.elem, direction, generics, depth)),
            Type::Slice(slice) => array_of(self.of(at, &slice.elem, direction, generics, depth)),
            Type::Tuple(tuple) if tuple.elems.is_empty() => ("null".to_string(), true),
            Type::Tuple(tuple) => {
                let parts: Vec<Ts> = tuple
                    .elems
                    .iter()
                    .map(|elem| self.of(at, elem, direction, generics, depth))
                    .collect();
                tuple_of(&parts)
            }
            Type::Path(path) if path.qself.is_none() => {
                self.path(at, &path.path, direction, generics, depth)
            }
            _ => unknown(),
        }
    }

    fn path(
        &mut self,
        at: &Module,
        path: &syn::Path,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let Some(last) = path.segments.last() else {
            return unknown();
        };
        if segments.len() == 1 && generics.contains(&segments[0]) {
            return (segments[0].clone(), true);
        }
        let args: Vec<Ts> = match &last.arguments {
            PathArguments::AngleBracketed(arguments) => arguments
                .args
                .iter()
                .filter_map(|argument| match argument {
                    GenericArgument::Type(ty) => {
                        Some(self.of(at, ty, direction, generics, depth + 1))
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let name = last.ident.to_string();
        let standard = matches!(segments[0].as_str(), "std" | "core" | "alloc");
        if (segments.len() == 1 || standard)
            && let Some(known) = builtin(&name, &args)
        {
            return known;
        }
        match self.locate(at, &segments, 0) {
            Located::Item(module, item) => self.reference(&module, &item, args, direction, depth),
            Located::External(full) => {
                let name = full.last().cloned().unwrap_or(name);
                external(&name, &args)
            }
        }
    }

    // ----- every struct a generated module can name -----------------------

    /// Each struct of the crate that a module generated inside `crate::routes`
    /// can name and whose JSON is known, as `(path, type)`:
    /// `("crate::services::projects::Project", "Project")`.
    ///
    /// The path is written into generated code, so a struct that might not
    /// resolve there is left out: a private one below the crate root or the
    /// routes module, one inside a module that is not `pub` or `pub(crate)`,
    /// anything behind a `#[cfg]`, and a generic struct.
    pub fn reachable(&mut self) -> Vec<(String, String)> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        self.walk(&root, &mut Vec::new(), 0, &mut paths);
        let mut out = Vec::new();
        for path in paths {
            let Ok(ty) = syn::parse_str::<Type>(&path) else {
                continue;
            };
            let shape = self.shape(&ty, Direction::Send);
            if shape.status == Status::Resolved {
                out.push((path, shape.typescript));
            }
        }
        out
    }

    fn walk(
        &mut self,
        module: &Module,
        path: &mut Vec<String>,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        // A private item is visible to the modules below the one it is
        // declared in: the generated module sits below `crate::routes`.
        let open = path.is_empty() || path.as_slice() == ["routes"];
        let listed = self.items(module, |items| {
            let gated = |attrs: &[Attribute]| attrs.iter().any(|attr| attr.path().is_ident("cfg"));
            let visible = |vis: &syn::Visibility| match vis {
                syn::Visibility::Public(_) => true,
                syn::Visibility::Restricted(restricted) => restricted.path.is_ident("crate"),
                syn::Visibility::Inherited => open,
            };
            let mut structs = Vec::new();
            let mut modules = Vec::new();
            for item in items {
                match item {
                    Item::Struct(item)
                        if !gated(&item.attrs)
                            && visible(&item.vis)
                            && item.generics.params.is_empty()
                            && matches!(item.fields, syn::Fields::Named(_)) =>
                    {
                        structs.push(item.ident.to_string());
                    }
                    Item::Mod(item) if !gated(&item.attrs) && visible(&item.vis) => {
                        modules.push(item.ident.to_string());
                    }
                    _ => {}
                }
            }
            (structs, modules)
        });
        let Some((structs, modules)) = listed else {
            return;
        };
        let prefix = std::iter::once("crate")
            .chain(path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("::");
        out.extend(structs.into_iter().map(|name| format!("{prefix}::{name}")));
        for name in modules {
            if let Some(child) = self.child(module, &name) {
                path.push(name);
                self.walk(&child, path, depth + 1, out);
                path.pop();
            }
        }
    }

    /// Whether the crate root this scope found declares `mod routes`.
    pub fn has_routes(&mut self) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        self.items(&root, |items| {
            items
                .iter()
                .any(|item| matches!(item, Item::Mod(module) if module.ident == "routes"))
        })
        .unwrap_or(false)
    }

    // ----- finding a definition ------------------------------------------

    fn items<R>(&mut self, module: &Module, read: impl FnOnce(&[Item]) -> R) -> Option<R> {
        let file = self.sources.file(&module.file)?;
        let mut items: &[Item] = &file.items;
        for name in &module.mods {
            items = items.iter().find_map(|item| match item {
                Item::Mod(inner) if inner.ident == name => {
                    inner.content.as_ref().map(|(_, items)| items.as_slice())
                }
                _ => None,
            })?;
        }
        Some(read(items))
    }

    fn definition(&mut self, module: &Module, name: &str) -> Option<Item> {
        let functions = self.functions;
        self.items(module, |items| {
            items
                .iter()
                .find(|item| match item {
                    Item::Fn(item) => functions && item.sig.ident == name,
                    Item::Struct(item) => !functions && item.ident == name,
                    Item::Enum(item) => !functions && item.ident == name,
                    Item::Type(item) => !functions && item.ident == name,
                    _ => false,
                })
                .cloned()
        })
        .flatten()
    }

    /// The file a component is declared in, when `name`, written in this
    /// scope's file, is a `#[component]` function of this crate: one the
    /// file declares, or one a `use` brings in.
    pub fn component_file(&mut self, name: &str) -> Option<PathBuf> {
        let home = self.home.clone();
        self.functions = true;
        let located = self.locate(&home, &[name.to_string()], 0);
        self.functions = false;
        match located {
            Located::Item(module, item) => match *item {
                Item::Fn(function)
                    if function
                        .attrs
                        .iter()
                        .any(|attr| attr.path().is_ident("component")) =>
                {
                    Some(module.file)
                }
                _ => None,
            },
            Located::External(_) => None,
        }
    }

    /// The directory a module's file-based children live in.
    fn children_dir(module: &Module) -> PathBuf {
        let parent = module.file.parent().unwrap_or(Path::new("")).to_path_buf();
        let stem = module.file.file_stem().and_then(|stem| stem.to_str());
        let mut dir = match stem {
            Some("mod" | "lib" | "main") | None => parent,
            Some(stem) => parent.join(stem),
        };
        dir.extend(&module.mods);
        dir
    }

    fn child(&mut self, module: &Module, name: &str) -> Option<Module> {
        // (inline, `#[path]`) for a `mod name` declared here.
        let declared = self
            .items(module, |items| {
                items.iter().find_map(|item| match item {
                    Item::Mod(inner) if inner.ident == name => Some((
                        inner.content.is_some(),
                        string_attribute(&inner.attrs, "path"),
                    )),
                    _ => None,
                })
            })
            .flatten();
        if let Some((true, _)) = declared {
            let mut mods = module.mods.clone();
            mods.push(name.to_string());
            return Some(Module {
                file: module.file.clone(),
                mods,
            });
        }
        let mut candidates = Vec::new();
        if let Some((_, Some(path))) = declared {
            candidates.push(module.file.parent().unwrap_or(Path::new("")).join(path));
        }
        let dir = Self::children_dir(module);
        candidates.push(dir.join(format!("{name}.rs")));
        candidates.push(dir.join(name).join("mod.rs"));
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .map(|file| Module {
                file,
                mods: Vec::new(),
            })
    }

    fn parent(&self, module: &Module) -> Option<Module> {
        if !module.mods.is_empty() {
            let mut mods = module.mods.clone();
            mods.pop();
            return Some(Module {
                file: module.file.clone(),
                mods,
            });
        }
        let own = module.file.parent()?;
        // The directory whose owner is this module's parent.
        let dir = match module.file.file_stem()?.to_str()? {
            "lib" | "main" => return None,
            "mod" => own.parent()?,
            _ => own,
        };
        let mut candidates = vec![dir.join("mod.rs"), dir.join("lib.rs"), dir.join("main.rs")];
        if let (Some(above), Some(name)) = (dir.parent(), dir.file_name().and_then(|n| n.to_str()))
        {
            candidates.push(above.join(format!("{name}.rs")));
        }
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .map(|file| Module {
                file,
                mods: Vec::new(),
            })
    }

    /// What `segments`, written in `module`, names.
    fn locate(&mut self, module: &Module, segments: &[String], depth: usize) -> Located {
        let external = || Located::External(segments.to_vec());
        let Some((first, rest)) = segments.split_first() else {
            return external();
        };
        if depth > MAX_DEPTH {
            return external();
        }
        let next = match first.as_str() {
            "crate" => Some(self.root.clone()),
            "self" => Some(Some(module.clone())),
            "super" => Some(self.parent(module)),
            _ => None,
        };
        if let Some(next) = next {
            return match next {
                Some(next) if !rest.is_empty() => self.locate(&next, rest, depth + 1),
                _ => external(),
            };
        }
        if rest.is_empty() {
            if let Some(item) = self.definition(module, first) {
                return Located::Item(module.clone(), Box::new(item));
            }
        } else if let Some(child) = self.child(module, first) {
            return self.locate(&child, rest, depth + 1);
        }
        let (named, globs) = self
            .items(module, |items| {
                let mut named = Vec::new();
                let mut globs = Vec::new();
                for item in items {
                    if let Item::Use(item) = item {
                        imports(&item.tree, &mut Vec::new(), &mut named, &mut globs);
                    }
                }
                (named, globs)
            })
            .unwrap_or_default();
        if let Some((_, path)) = named.iter().find(|(name, _)| name == first) {
            // `use serde_json::Value;` names itself once the crate is left.
            if path.as_slice() == segments {
                return external();
            }
            let mut full = path.clone();
            full.extend_from_slice(rest);
            return self.locate(module, &full, depth + 1);
        }
        for glob in globs {
            let mut full = glob;
            full.extend_from_slice(segments);
            if let found @ Located::Item(..) = self.locate(module, &full, depth + 1) {
                return found;
            }
        }
        external()
    }

    // ----- declaring a named type ----------------------------------------

    fn reference(
        &mut self,
        module: &Module,
        item: &Item,
        args: Vec<Ts>,
        direction: Direction,
        depth: usize,
    ) -> Ts {
        let (ident, attrs, generics) = match item {
            Item::Struct(item) => (&item.ident, &item.attrs, &item.generics),
            Item::Enum(item) => (&item.ident, &item.attrs, &item.generics),
            Item::Type(item) => (&item.ident, &item.attrs, &item.generics),
            _ => return unknown(),
        };
        // A hand-written impl decides its own JSON, which the fields do not say.
        if !matches!(item, Item::Type(_)) && !derives(attrs, direction) {
            return unknown();
        }
        let ident = ident.to_string();
        let key = (module.clone(), ident.clone(), direction);
        let name = match self.names.get(&key) {
            Some(name) => name.clone(),
            None => {
                let name = self.unused(&ident, direction);
                let params: Vec<String> = generics
                    .type_params()
                    .map(|param| param.ident.to_string())
                    .collect();
                self.names.insert(key, name.clone());
                // Assumed while the body is read, so a type that contains
                // itself refers to its own name.
                self.complete.insert(name.clone(), true);
                self.arity.insert(name.clone(), params.len());
                let (body, complete) = self.body(module, item, direction, &params, depth + 1);
                let head = |name: &str| {
                    if params.is_empty() {
                        name.to_string()
                    } else {
                        format!("{name}<{}>", params.join(", "))
                    }
                };
                // Most types read the same in both directions, and are then
                // one declaration rather than two under different names.
                let other = match direction {
                    Direction::Send => Direction::Receive,
                    Direction::Receive => Direction::Send,
                };
                let twin = self
                    .names
                    .get(&(module.clone(), ident.clone(), other))
                    .filter(|twin| {
                        self.types.get(&head(twin.as_str()))
                            == Some(&body.replace(&name, twin.as_str()))
                    })
                    .cloned();
                if let Some(twin) = twin {
                    self.complete.remove(&name);
                    self.arity.remove(&name);
                    self.names
                        .insert((module.clone(), ident, direction), twin.clone());
                    twin
                } else {
                    self.complete.insert(name.clone(), complete);
                    self.types.insert(head(&name), body);
                    name
                }
            }
        };
        let arity = self.arity[&name];
        let mut complete = self.complete[&name];
        if arity == 0 {
            return (name, complete);
        }
        let mut written = Vec::new();
        for index in 0..arity {
            let (text, ok) = args.get(index).cloned().unwrap_or_else(unknown);
            complete &= ok;
            written.push(text);
        }
        (format!("{name}<{}>", written.join(", ")), complete)
    }

    fn unused(&self, ident: &str, direction: Direction) -> String {
        let taken = |name: &str| self.arity.contains_key(name);
        if !taken(ident) {
            return ident.to_string();
        }
        if direction == Direction::Receive {
            let input = format!("{ident}Input");
            if !taken(&input) {
                return input;
            }
        }
        (2..)
            .map(|n| format!("{ident}_{n}"))
            .find(|name| !taken(name))
            .unwrap()
    }

    fn body(
        &mut self,
        module: &Module,
        item: &Item,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        match item {
            Item::Type(alias) => self.of(module, &alias.ty, direction, generics, depth),
            Item::Struct(item) => {
                let serde = Serde::read(&item.attrs, direction);
                if let Some(other) = &serde.convert {
                    return self.converted(module, other, direction, generics, depth);
                }
                self.fields(
                    module,
                    &item.fields,
                    &serde,
                    None,
                    direction,
                    generics,
                    depth,
                )
            }
            Item::Enum(item) => {
                let serde = Serde::read(&item.attrs, direction);
                if let Some(other) = &serde.convert {
                    return self.converted(module, other, direction, generics, depth);
                }
                let mut complete = true;
                let mut members = Vec::new();
                for variant in &item.variants {
                    let own = Serde::read(&variant.attrs, direction);
                    if own.skip {
                        continue;
                    }
                    if own.other {
                        members.push("string".to_string());
                        continue;
                    }
                    let name = own.rename.clone().unwrap_or_else(|| {
                        rename_variant(&variant.ident.to_string(), serde.rename_all.as_deref())
                    });
                    let tag = serde_json::to_string(&name).unwrap();
                    let unit = matches!(variant.fields, Fields::Unit);
                    let rule = Serde {
                        rename_all: own.rename_all.clone().or(serde.rename_all_fields.clone()),
                        ..Serde::default()
                    };
                    let untagged = serde.untagged || own.untagged;
                    let internal = match (&serde.tag, &serde.content, untagged) {
                        (Some(tag_key), None, false) if !unit => Some((tag_key, &tag)),
                        _ => None,
                    };
                    let (content, ok) = if unit {
                        ("null".to_string(), true)
                    } else {
                        self.fields(
                            module,
                            &variant.fields,
                            &rule,
                            internal,
                            direction,
                            generics,
                            depth,
                        )
                    };
                    complete &= ok;
                    members.push(match (&serde.tag, &serde.content, untagged, unit) {
                        (_, _, true, _) => content,
                        (None, _, _, true) => tag,
                        (None, _, _, false) => format!("{{ {}: {content} }}", property(&name)),
                        (Some(key), _, _, true) => format!("{{ {}: {tag} }}", property(key)),
                        (Some(_), None, _, false) => content,
                        (Some(key), Some(inner), _, false) => format!(
                            "{{ {}: {tag}; {}: {content} }}",
                            property(key),
                            property(inner)
                        ),
                    });
                }
                if members.is_empty() {
                    return ("never".to_string(), complete);
                }
                (members.join(" | "), complete)
            }
            _ => unknown(),
        }
    }

    /// `#[serde(into = "Wire")]`: the JSON is `Wire`'s.
    fn converted(
        &mut self,
        module: &Module,
        other: &str,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        match syn::parse_str::<Type>(other) {
            Ok(ty) => self.of(module, &ty, direction, generics, depth),
            Err(_) => unknown(),
        }
    }

    /// A struct's or a variant's fields. `tag` is an internally tagged
    /// enum's `(key, "Variant")`, written among the fields.
    #[allow(clippy::too_many_arguments)]
    fn fields(
        &mut self,
        module: &Module,
        fields: &Fields,
        container: &Serde,
        tag: Option<(&String, &String)>,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        let tagged = tag.map(|(key, value)| format!("{}: {value}", property(key)));
        match fields {
            Fields::Unit => ("null".to_string(), true),
            Fields::Unnamed(unnamed) => {
                let kept: Vec<&syn::Field> = unnamed
                    .unnamed
                    .iter()
                    .filter(|field| !Serde::read(&field.attrs, direction).skip)
                    .collect();
                let mut parts = Vec::new();
                for field in &kept {
                    parts.push(self.field(module, field, direction, generics, depth));
                }
                let (text, complete) = match parts.len() {
                    1 => parts.remove(0),
                    _ => tuple_of(&parts),
                };
                match tagged {
                    Some(tagged) => (format!("{{ {tagged} }} & {text}"), complete),
                    None => (text, complete),
                }
            }
            Fields::Named(named) => {
                let mut complete = true;
                let mut properties: Vec<String> = tagged.into_iter().collect();
                let mut flattened = Vec::new();
                let mut only = None;
                for field in &named.named {
                    let serde = Serde::read(&field.attrs, direction);
                    if serde.skip {
                        continue;
                    }
                    let (text, ok) = self.field(module, field, direction, generics, depth);
                    complete &= ok;
                    if container.transparent {
                        only = Some(text);
                        continue;
                    }
                    if serde.flatten {
                        flattened.push(text);
                        continue;
                    }
                    let ident = field.ident.as_ref().map(|ident| ident.to_string());
                    let ident = ident.unwrap_or_default();
                    let ident = ident.trim_start_matches("r#");
                    let name = serde
                        .rename
                        .clone()
                        .unwrap_or_else(|| rename_field(ident, container.rename_all.as_deref()));
                    let optional = match direction {
                        Direction::Send => serde.skipped_when,
                        Direction::Receive => {
                            serde.default || container.default || is_option(&field.ty)
                        }
                    };
                    let mark = if optional { "?" } else { "" };
                    properties.push(format!("{}{mark}: {text}", property(&name)));
                }
                if let Some(only) = only {
                    return (only, complete);
                }
                let object = if properties.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{ {} }}", properties.join("; "))
                };
                if flattened.is_empty() {
                    return (object, complete);
                }
                let mut parts = Vec::new();
                if !properties.is_empty() {
                    parts.push(object);
                }
                parts.extend(flattened.into_iter().map(|text| {
                    if text.contains(" | ") {
                        format!("({text})")
                    } else {
                        text
                    }
                }));
                (parts.join(" & "), complete)
            }
        }
    }

    fn field(
        &mut self,
        module: &Module,
        field: &syn::Field,
        direction: Direction,
        generics: &[String],
        depth: usize,
    ) -> Ts {
        // `with = "…"` writes JSON the field's type does not describe.
        if Serde::read(&field.attrs, direction).custom {
            return unknown();
        }
        self.of(module, &field.ty, direction, generics, depth)
    }
}

// ----- known types -------------------------------------------------------

fn first(args: &[Ts]) -> Ts {
    args.first().cloned().unwrap_or_else(unknown)
}

fn array_of((text, complete): Ts) -> Ts {
    if text.contains(" | ") || text.contains(" & ") {
        (format!("Array<{text}>"), complete)
    } else {
        (format!("{text}[]"), complete)
    }
}

fn tuple_of(parts: &[Ts]) -> Ts {
    let texts: Vec<&str> = parts.iter().map(|(text, _)| text.as_str()).collect();
    (
        format!("[{}]", texts.join(", ")),
        parts.iter().all(|(_, complete)| *complete),
    )
}

/// Standard library types, by the name they are written with.
fn builtin(name: &str, args: &[Ts]) -> Option<Ts> {
    let text = |text: &str| Some((text.to_string(), true));
    match name {
        "String" | "str" | "char" | "PathBuf" | "Path" | "OsString" => text("string"),
        "bool" => text("boolean"),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" | "NonZeroU8" | "NonZeroU16" | "NonZeroU32" | "NonZeroU64"
        | "NonZeroUsize" | "NonZeroI8" | "NonZeroI16" | "NonZeroI32" | "NonZeroI64"
        | "NonZeroIsize" => text("number"),
        "Option" => {
            let (inner, complete) = first(args);
            Some((format!("{inner} | null"), complete))
        }
        "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "LinkedList" | "BinaryHeap" => {
            Some(array_of(first(args)))
        }
        "Box" | "Rc" | "Arc" | "Cow" | "Cell" | "RefCell" | "Mutex" | "RwLock" => Some(first(args)),
        "HashMap" | "BTreeMap" => {
            let (value, complete) = args.get(1).cloned().unwrap_or_else(unknown);
            Some((format!("Record<string, {value}>"), complete))
        }
        _ => None,
    }
}

/// A type the crate's own source does not define, by its last name. These
/// are the JSON forms of the crates a Rahti application is scaffolded with.
fn external(name: &str, args: &[Ts]) -> Ts {
    match name {
        "Value" | "Json" | "JsonValue" => ("unknown".to_string(), true),
        "DateTime"
        | "DateTimeUtc"
        | "DateTimeWithTimeZone"
        | "DateTimeLocal"
        | "NaiveDate"
        | "NaiveDateTime"
        | "NaiveTime"
        | "Date"
        | "Time"
        | "TimeDate"
        | "TimeTime"
        | "TimeDateTime"
        | "TimeDateTimeWithTimeZone"
        | "Uuid"
        | "Decimal"
        | "BigDecimal"
        | "Url"
        | "IpAddr"
        | "Ipv4Addr"
        | "Ipv6Addr"
        | "SocketAddr" => ("string".to_string(), true),
        "RpcFile" | "RpcUpload" => ("File".to_string(), true),
        "IndexMap" => builtin("HashMap", args).unwrap_or_else(unknown),
        "IndexSet" => builtin("Vec", args).unwrap_or_else(unknown),
        _ => builtin(name, args).unwrap_or_else(unknown),
    }
}

fn is_option(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|s| s.ident == "Option"))
}

/// A property name as TypeScript writes it.
fn property(name: &str) -> String {
    let mut chars = name.chars();
    let plain = matches!(chars.next(), Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric());
    if plain {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap()
    }
}

// ----- `use` declarations ------------------------------------------------

fn imports(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    named: &mut Vec<(String, Vec<String>)>,
    globs: &mut Vec<Vec<String>>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            imports(&path.tree, prefix, named, globs);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let ident = name.ident.to_string();
            let mut full = prefix.clone();
            // `use a::b::{self}` binds `b`.
            if ident == "self" {
                if let Some(last) = prefix.last() {
                    named.push((last.clone(), full));
                }
            } else {
                full.push(ident.clone());
                named.push((ident, full));
            }
        }
        UseTree::Rename(rename) => {
            let mut full = prefix.clone();
            if rename.ident != "self" {
                full.push(rename.ident.to_string());
            }
            named.push((rename.rename.to_string(), full));
        }
        UseTree::Glob(_) => globs.push(prefix.clone()),
        UseTree::Group(group) => {
            for tree in &group.items {
                imports(tree, prefix, named, globs);
            }
        }
    }
}

// ----- attributes --------------------------------------------------------

fn string_attribute(attrs: &[Attribute], name: &str) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident(name) {
            return None;
        }
        match &attr.meta.require_name_value().ok()?.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) => Some(text.value()),
            _ => None,
        }
    })
}

/// Whether the item derives the Serde trait this direction uses.
fn derives(attrs: &[Attribute], direction: Direction) -> bool {
    let wanted = match direction {
        Direction::Send => "Serialize",
        Direction::Receive => "Deserialize",
    };
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive") || attr.path().is_ident("cfg_attr"))
        .any(|attr| {
            quote::ToTokens::to_token_stream(&attr.meta)
                .to_string()
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|word| word == wanted)
        })
}

/// The `#[serde(…)]` attributes on one item, field or variant that change
/// its JSON in one direction.
#[derive(Default)]
struct Serde {
    rename: Option<String>,
    rename_all: Option<String>,
    rename_all_fields: Option<String>,
    skip: bool,
    /// `skip_serializing_if`: the key may be absent.
    skipped_when: bool,
    default: bool,
    flatten: bool,
    transparent: bool,
    untagged: bool,
    other: bool,
    tag: Option<String>,
    content: Option<String>,
    /// `with`, `serialize_with`, `deserialize_with`.
    custom: bool,
    /// `into`, or `from`/`try_from`.
    convert: Option<String>,
}

impl Serde {
    fn read(attrs: &[Attribute], direction: Direction) -> Serde {
        let mut out = Serde::default();
        let send = direction == Direction::Send;
        let side = if send { "serialize" } else { "deserialize" };
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
            // An attribute that does not parse changes nothing we can read.
            let _ = attr.parse_nested_meta(|meta| {
                let key = meta
                    .path
                    .get_ident()
                    .map(|ident| ident.to_string())
                    .unwrap_or_default();
                // `rename(serialize = "a", deserialize = "b")`.
                if meta.input.peek(syn::token::Paren) {
                    let mut value = None;
                    meta.parse_nested_meta(|inner| {
                        let text: syn::LitStr = inner.value()?.parse()?;
                        if inner.path.is_ident(side) {
                            value = Some(text.value());
                        }
                        Ok(())
                    })?;
                    match (key.as_str(), value) {
                        ("rename", Some(value)) => out.rename = Some(value),
                        ("rename_all", Some(value)) => out.rename_all = Some(value),
                        ("rename_all_fields", Some(value)) => out.rename_all_fields = Some(value),
                        _ => {}
                    }
                    return Ok(());
                }
                let value = if meta.input.peek(syn::Token![=]) {
                    match meta.value()?.parse::<syn::Expr>()? {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(text),
                            ..
                        }) => Some(text.value()),
                        _ => None,
                    }
                } else {
                    None
                };
                match key.as_str() {
                    "rename" => out.rename = value,
                    "rename_all" => out.rename_all = value,
                    "rename_all_fields" => out.rename_all_fields = value,
                    "skip" => out.skip = true,
                    "skip_serializing" if send => out.skip = true,
                    "skip_deserializing" if !send => out.skip = true,
                    "skip_serializing_if" => out.skipped_when = true,
                    "default" => out.default = true,
                    "flatten" => out.flatten = true,
                    "transparent" => out.transparent = true,
                    "untagged" => out.untagged = true,
                    "other" => out.other = true,
                    "tag" => out.tag = value,
                    "content" => out.content = value,
                    "with" => out.custom = true,
                    "serialize_with" if send => out.custom = true,
                    "deserialize_with" if !send => out.custom = true,
                    "into" if send => out.convert = value,
                    "from" | "try_from" if !send => out.convert = value,
                    _ => {}
                }
                Ok(())
            });
        }
        out
    }
}

// ----- `rename_all` ------------------------------------------------------

/// A field name, written in snake_case, under a `rename_all` rule.
fn rename_field(name: &str, rule: Option<&str>) -> String {
    let pascal = || {
        let mut out = String::new();
        let mut upper = true;
        for c in name.chars() {
            if c == '_' {
                upper = true;
            } else if upper {
                out.push(c.to_ascii_uppercase());
                upper = false;
            } else {
                out.push(c);
            }
        }
        out
    };
    match rule {
        Some("UPPERCASE" | "SCREAMING_SNAKE_CASE") => name.to_ascii_uppercase(),
        Some("PascalCase") => pascal(),
        Some("camelCase") => lower_first(&pascal()),
        Some("kebab-case") => name.replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => name.to_ascii_uppercase().replace('_', "-"),
        _ => name.to_string(),
    }
}

/// A variant name, written in PascalCase, under a `rename_all` rule.
fn rename_variant(name: &str, rule: Option<&str>) -> String {
    let snake = || {
        let mut out = String::new();
        for (index, c) in name.char_indices() {
            if index > 0 && c.is_uppercase() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    };
    match rule {
        Some("lowercase") => name.to_ascii_lowercase(),
        Some("UPPERCASE") => name.to_ascii_uppercase(),
        Some("camelCase") => lower_first(name),
        Some("snake_case") => snake(),
        Some("SCREAMING_SNAKE_CASE") => snake().to_ascii_uppercase(),
        Some("kebab-case") => snake().replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => snake().to_ascii_uppercase().replace('_', "-"),
        _ => name.to_string(),
    }
}

fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "tests/shapes.rs"]
mod tests;
