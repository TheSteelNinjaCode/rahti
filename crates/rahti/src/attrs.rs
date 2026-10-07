//! Attributes that are decided at runtime: whether one is written at all, and
//! what a spread brings with it.
//!
//! Most of an open tag is known when it is typed, and `html!` folds that into
//! one `push_str`. Two things are not, and both arrive here.
//!
//! The first is **presence**. JSX omits an attribute whose value is `undefined`
//! and omits a boolean one whose value is `false`; markup that always writes
//! the attribute cannot say either. `href=""` is a link to the current page,
//! and `disabled="false"` disables the control, because HTML reads a boolean
//! attribute by whether it is there. So a value decides, through
//! [`Render::attribute_presence`], and [`attribute`] acts on the answer.
//!
//! The second is **spread**. `<svg class="…" @{..attrs}>` takes a set of
//! attributes computed in Rust, and neither the names nor how many there are
//! can be spelled in a literal tag. An element carrying one builds its open tag
//! through an [`AttrList`] instead, which is also what gives a spread the
//! last-wins behaviour `Object.assign` has in JSX: whatever is written later
//! replaces what came before, in place, rather than emitting the same attribute
//! twice for the browser to pick the first of.

use std::collections::{BTreeMap, HashMap};

use crate::html::{Render, escape_attr_into, escape_source_attr_into};

#[cfg(test)]
#[path = "tests/attrs.rs"]
mod tests;

/// The framework's own attribute, which a spread does not get to write.
const COMPONENT_ATTR: &str = "pp-component";

/// What a value makes of the attribute it was written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// `name="…"`, the value rendered inside the quotes. The answer for
    /// everything that is not an absent optional or a boolean flag.
    Value,
    /// `name=""` — present, with nothing in it. What a boolean attribute set
    /// to `true` renders, the same as JSX.
    Empty,
    /// Nothing at all: no name, no quotes, no space.
    Absent,
}

/// Write ` name="value"` into `out`, or nothing, or ` name=""`.
///
/// `name` carries its own leading space, so the caller does not decide
/// spacing for an attribute that may not be written. `boolean` is true when
/// the attribute is one HTML reads by presence — see the table in
/// `rahti-macros`, which is where the decision is made, at compile time, from
/// the name the author wrote.
pub fn attribute<T: Render + ?Sized>(name: &str, value: &T, boolean: bool, out: &mut String) {
    match value.attribute_presence(boolean) {
        Presence::Absent => {}
        Presence::Empty => {
            out.push_str(name);
            out.push_str("=\"\"");
        }
        Presence::Value => {
            out.push_str(name);
            out.push_str("=\"");
            value.render_attr_into(out);
            out.push('"');
        }
    }
}

// ------------------------------------------------------------------- spread

/// A set of attributes built in Rust and spread onto an element.
///
/// ```ignore
/// let extra = Attrs::new().set("id", "avatar").unset("stroke-width");
/// html! { <svg class="size-4" @{..extra}>"…"</svg> }
/// ```
///
/// [`set`](Attrs::set) writes an attribute, replacing one the tag already had;
/// [`unset`](Attrs::unset) takes one away, which no attribute value can
/// express on its own.
///
/// A name is written into markup verbatim, so it has to be one markup can
/// carry. A name that is not is dropped rather than written, and trips a
/// `debug_assert!` on the way out: development says so loudly, and a release
/// build serves the page without that attribute rather than serving markup
/// that no longer parses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs(Vec<(String, Option<String>, bool)>);

impl Attrs {
    pub fn new() -> Self {
        Attrs(Vec::new())
    }

    /// Write `name="value"`. Naming one twice keeps the last value and the
    /// first position, so a set built in a loop reads the way the loop wrote
    /// it.
    #[must_use = "Attrs::set returns a new value; it does not modify in place"]
    pub fn set(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.put(name.into(), Some(value.into()), false)
    }

    /// Write `name="{expr}"`: a PulsePoint binding the component composes
    /// itself, live in the browser.
    ///
    /// A [`set`](Attrs::set) value is data, and its braces are escaped so no
    /// stored value can open a client expression. This is the one way to write
    /// client code into a set from Rust — a handler, `pp.props.checkedProp`, a
    /// caller's binding under a new name — so like
    /// [`Html::from_raw`](crate::Html::from_raw) it is for code the author
    /// wrote, never for a value that came from a request. A tag's own `{…}`
    /// props arrive already live, as [`Bindings`].
    #[must_use = "Attrs::bind returns a new value; it does not modify in place"]
    pub fn bind(self, name: impl Into<String>, expr: impl AsRef<str>) -> Self {
        let source = format!("{{{}}}", expr.as_ref());
        self.put(name.into(), Some(source), true)
    }

    /// Whether this set writes `name` as a binding, through
    /// [`bind`](Attrs::bind).
    pub fn is_bound(&self, name: &str) -> bool {
        self.0
            .iter()
            .any(|(existing, value, bound)| existing == name && value.is_some() && *bound)
    }

    /// Take an attribute off the element.
    #[must_use = "Attrs::unset returns a new value; it does not modify in place"]
    pub fn unset(self, name: impl Into<String>) -> Self {
        self.put(name.into(), None, false)
    }

    /// Lay `over` on this set, last-wins: each of its values replaces one
    /// here in place, each removal removes, and a binding stays a binding.
    /// The way a component folds a caller's set into its own — rebuilding
    /// one from [`iter`](Attrs::iter) and [`set`](Attrs::set) would turn every
    /// binding into text.
    #[must_use = "Attrs::merge returns a new value; it does not modify in place"]
    pub fn merge(self, over: &Attrs) -> Self {
        over.0.iter().fold(self, |attrs, (name, value, bound)| {
            attrs.put(name.clone(), value.clone(), *bound)
        })
    }

    /// The entries whose name `keep` accepts, in order, bindings still
    /// bindings — for a component that splits a caller's set between two
    /// elements: `attrs.only(|name| name.starts_with("aria-"))`.
    #[must_use = "Attrs::only returns a new value; it does not modify in place"]
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> Self {
        Attrs(
            self.0
                .iter()
                .filter(|(name, _, _)| keep(name))
                .cloned()
                .collect(),
        )
    }

    fn put(mut self, name: String, value: Option<String>, bound: bool) -> Self {
        match self.0.iter_mut().find(|(existing, _, _)| *existing == name) {
            Some(entry) => {
                entry.1 = value;
                entry.2 = bound;
            }
            None => self.0.push((name, value, bound)),
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// What this set says to do, in the order it was said: a value to write,
    /// or `None` to take the attribute away.
    ///
    /// A set is built to be spread, and spreading it is usually the end of
    /// it. This is for the code that has to *compose* one — a component that
    /// merges a caller's attributes into its own has to know what the caller
    /// named before it can decide what to add.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.0
            .iter()
            .map(|(name, value, _)| (name.as_str(), value.as_deref()))
    }

    /// The attributes this set touches, in order — whether it writes them or
    /// removes them.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _, _)| name.as_str())
    }

    /// What this set writes under `name`: `None` if it does not name it at
    /// all, `Some(None)` if it names it as a removal.
    pub fn get(&self, name: &str) -> Option<Option<&str>> {
        self.0
            .iter()
            .find(|(existing, _, _)| existing == name)
            .map(|(_, value, _)| value.as_deref())
    }
}

/// `Attrs::from([("id", "avatar"), ("data-user", "42")])` — the bulk
/// spelling, for a set known all at once.
impl<N: Into<String>, V: Into<String>, const K: usize> From<[(N, V); K]> for Attrs {
    fn from(pairs: [(N, V); K]) -> Self {
        pairs.into_iter().collect()
    }
}

/// The same, for a set computed in Rust — `map(…).collect()`.
impl<N: Into<String>, V: Into<String>> FromIterator<(N, V)> for Attrs {
    fn from_iter<I: IntoIterator<Item = (N, V)>>(pairs: I) -> Self {
        pairs
            .into_iter()
            .fold(Attrs::new(), |attrs, (name, value)| attrs.set(name, value))
    }
}

impl<'a> IntoIterator for &'a Attrs {
    type Item = (&'a str, Option<&'a str>);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, Option<String>, bool)>,
        fn(&'a (String, Option<String>, bool)) -> (&'a str, Option<&'a str>),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0
            .iter()
            .map(|(name, value, _)| (name.as_str(), value.as_deref()))
    }
}

/// Something `@{..expr}` can spread onto an element.
///
/// Implemented for [`Attrs`], for pairs in an array, slice, `Vec` or map, and
/// for an `Option` of any of them — so a spread that is sometimes nothing is
/// `@{..maybe}` rather than a branch around the whole tag.
pub trait Attributes {
    fn write_attributes(&self, list: &mut AttrList);
}

impl Attributes for Attrs {
    fn write_attributes(&self, list: &mut AttrList) {
        for (name, value, bound) in &self.0 {
            match value {
                Some(value) if *bound => list.set_source(name, value),
                Some(value) => list.set(name, value),
                None => list.unset(name),
            }
        }
    }
}

/// The `{…}` props a component tag wrote, as source. `<Button
/// onclick={save()}>` hands the `Button` function `("onclick", "save()")`,
/// and spreading them — `@{..bindings}` — puts the handler on
/// the element the component renders, where the runtime runs it in the
/// caller's scope; a value binding, `class="{active}"`, lands in braces.
///
/// A component takes them as a `bindings: Bindings` parameter. Only `html!`
/// builds one, from the literal source at the tag, which is why its braces
/// are written live where every other spread's values are data and have
/// theirs escaped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bindings(&'static [(&'static str, &'static str)]);

impl Bindings {
    #[doc(hidden)]
    pub const fn __from_html(source: &'static [(&'static str, &'static str)]) -> Self {
        Bindings(source)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each binding as `(name, source)`: the expression the tag wrote
    /// between its braces.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &'static str)> {
        self.0.iter().copied()
    }
}

/// A binding goes back on an element in the form an element writes it: a
/// native handler is JavaScript, `onclick="save()"`, and everything else is
/// an expression in braces, `class="{active}"`. Without the braces a value
/// binding would land as the static text of its own source.
impl Attributes for Bindings {
    fn write_attributes(&self, list: &mut AttrList) {
        for (name, source) in self.0 {
            if is_native_event_name(name) {
                list.set_source(name, source);
            } else {
                list.set_source(name, &format!("{{{source}}}"));
            }
        }
    }
}

impl<T: Attributes + ?Sized> Attributes for &T {
    fn write_attributes(&self, list: &mut AttrList) {
        (**self).write_attributes(list);
    }
}

/// Present, or nothing at all — the spread that is sometimes not there.
impl<T: Attributes> Attributes for Option<T> {
    fn write_attributes(&self, list: &mut AttrList) {
        if let Some(attrs) = self {
            attrs.write_attributes(list);
        }
    }
}

macro_rules! attributes_for_pairs {
    ($($ty:ty),* $(,)?) => {
        $(impl<K: AsRef<str>, V: AsRef<str>> Attributes for $ty {
            fn write_attributes(&self, list: &mut AttrList) {
                for (name, value) in self.iter() {
                    list.set(name.as_ref(), value.as_ref());
                }
            }
        })*
    };
}

attributes_for_pairs!([(K, V)], Vec<(K, V)>);

impl<K: AsRef<str>, V: AsRef<str>, const N: usize> Attributes for [(K, V); N] {
    fn write_attributes(&self, list: &mut AttrList) {
        for (name, value) in self {
            list.set(name.as_ref(), value.as_ref());
        }
    }
}

impl<K: AsRef<str>, V: AsRef<str>> Attributes for BTreeMap<K, V> {
    fn write_attributes(&self, list: &mut AttrList) {
        for (name, value) in self {
            list.set(name.as_ref(), value.as_ref());
        }
    }
}

/// A `HashMap` has no order of its own, so what it spreads is sorted — markup
/// that changes between two runs of the same code is markup nobody can diff.
impl<K: AsRef<str>, V: AsRef<str>> Attributes for HashMap<K, V> {
    fn write_attributes(&self, list: &mut AttrList) {
        let mut pairs: Vec<(&str, &str)> = self
            .iter()
            .map(|(name, value)| (name.as_ref(), value.as_ref()))
            .collect();
        pairs.sort_by_key(|(name, _)| *name);
        for (name, value) in pairs {
            list.set(name, value);
        }
    }
}

// ----------------------------------------------------------------- the list

/// An open tag under construction.
///
/// Built by `html!` for an element carrying a spread, and handed to every
/// [`Attributes`] impl along the way. One entry per name, in the order the
/// names first appeared, with the last value written for each — which is what
/// makes a spread after an attribute override it, and a spread before it a
/// set of defaults.
#[derive(Debug, Default)]
pub struct AttrList {
    entries: Vec<(String, String)>,
    /// Whether the writes arriving now came out of a spread rather than off
    /// the tag. Only [`COMPONENT_ATTR`] and, on a root that owns its events,
    /// native event handlers care, and only to refuse them.
    spreading: bool,
    /// The element is a block's root that handles its own events: `html!`
    /// named the block as their owner (`pp-event-owner`), so every native
    /// handler on it runs in the block's scope. See [`AttrList::own_events`].
    owns_events: bool,
}

impl AttrList {
    pub fn new() -> Self {
        AttrList {
            entries: Vec::new(),
            spreading: false,
            owns_events: false,
        }
    }

    /// Mark this open tag as a root whose handlers belong to its own block.
    ///
    /// Written by `html!` first, on a root that carries both a handler of
    /// its own (`onsubmit={save(event)}`) and a spread. One element has one
    /// event owner, so a handler the spread brings — a caller's, forwarded
    /// through `bindings` — would run in the wrong scope. It is refused the
    /// way a spread `pp-component` is: loudly in development, dropped in a
    /// release build rather than wired to the wrong function.
    pub fn own_events(&mut self) {
        self.owns_events = true;
    }

    /// Write an attribute from a string value. It is data: escaped here,
    /// braces included, so no value can open a client expression.
    ///
    /// A native handler (`onclick`) is refused: the runtime runs its value as
    /// JavaScript, which HTML escaping does not make safe. A caller's handler
    /// travels as [`Bindings`]. Loud in development, dropped in a release
    /// build, like the other spread refusals.
    pub fn set(&mut self, name: &str, value: &str) {
        if is_native_event_name(name) {
            debug_assert!(
                false,
                "rahti: a spread wrote `{name}` from a string value, and the runtime would run \
                 it as JavaScript. Forward a caller's handler with `bindings: Bindings` and \
                 `@{{..bindings}}`, or write the handler as a client binding: `{name}={{…}}`."
            );
            return;
        }
        let mut escaped = String::with_capacity(value.len());
        escape_attr_into(value, &mut escaped);
        self.put(name, escaped);
    }

    /// Write an attribute the author typed — a literal on the tag, or a
    /// [`Bindings`] entry. Escaped for the attribute, with its braces kept:
    /// they are a client `{…}` binding.
    pub fn set_source(&mut self, name: &str, source: &str) {
        let mut escaped = String::with_capacity(source.len());
        escape_source_attr_into(source, &mut escaped);
        self.put(name, escaped);
    }

    /// Take an attribute off, whatever wrote it.
    pub fn unset(&mut self, name: &str) {
        self.entries.retain(|(existing, _)| existing != name);
    }

    /// Write an attribute from an `@{…}` value, which decides for itself
    /// whether it is written at all. An absent one removes what came before,
    /// so `@{..attrs} hidden=@{false}` ends up not hidden.
    pub fn value<T: Render + ?Sized>(&mut self, name: &str, value: &T, boolean: bool) {
        match value.attribute_presence(boolean) {
            Presence::Absent => self.unset(name),
            Presence::Empty => self.put(name, String::new()),
            Presence::Value => {
                let mut rendered = String::new();
                value.render_attr_into(&mut rendered);
                self.put(name, rendered);
            }
        }
    }

    /// Fold a spread in at this point in the tag.
    pub fn spread<A: Attributes + ?Sized>(&mut self, attrs: &A) {
        let outer = std::mem::replace(&mut self.spreading, true);
        attrs.write_attributes(self);
        self.spreading = outer;
    }

    pub fn write_into(&self, out: &mut String) {
        for (name, value) in &self.entries {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(value);
            out.push('"');
        }
    }

    fn put(&mut self, name: &str, value: String) {
        if !is_valid_name(name) {
            debug_assert!(
                false,
                "rahti: `{name}` is not an attribute name markup can carry, so it is \
                 dropped rather than written into the open tag."
            );
            return;
        }
        if self.spreading && name == COMPONENT_ATTR {
            // The client runtime binds this name, and `html!` has already put
            // the block's own into the static part of the open tag. A spread
            // that wrote it too would name the block twice, from a value
            // nothing at the tag can see. Written on the tag it is honoured
            // as always — that is the documented way to pin a name.
            debug_assert!(
                false,
                "rahti: `{COMPONENT_ATTR}` is the client runtime's, and a spread does not \
                 write it. Put it on the tag literally to pin a block's name."
            );
            return;
        }
        if self.spreading && self.owns_events && is_native_event_name(name) {
            debug_assert!(
                false,
                "rahti: a spread brings `{name}` onto a root that handles its own events, \
                 and one element has one event owner — the root's handlers run in its own \
                 block's scope. Move `{name}` to an inner element, or call the caller's \
                 handler from the block's own."
            );
            return;
        }
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == name)
        {
            Some(entry) => entry.1 = value,
            None => self.entries.push((name.to_string(), value)),
        }
    }
}

/// Does this attribute name a native DOM event handler? The client runtime's
/// naming contract — lowercase, unhyphenated `on…`; `on-…` is a component
/// callback prop. Mirrors `is_native_event_name` in `rahti-macros`.
pub(crate) fn is_native_event_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.len() > 2 && lower.starts_with("on") && lower[2..].bytes().all(|b| b.is_ascii_lowercase())
}

/// Is this a name that can be written into an open tag?
///
/// Narrower than the HTML parser's rule, which takes nearly anything that is
/// not whitespace, `/`, `=`, `>` or a quote. The set here is what real
/// attributes are made of — including the `:` that `xlink:href` and
/// `xml:space` need — and refusing the rest means a value that wandered into
/// the name position cannot close the tag it was written in.
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}
