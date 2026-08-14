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

use crate::html::{Render, escape_attr_into};

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
/// html! { <svg class="size-4" @{..extra}>…</svg> }
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
pub struct Attrs(Vec<(String, Option<String>)>);

impl Attrs {
    pub fn new() -> Self {
        Attrs(Vec::new())
    }

    /// Write `name="value"`. Naming one twice keeps the last value and the
    /// first position, so a set built in a loop reads the way the loop wrote
    /// it.
    #[must_use = "Attrs::set returns a new value; it does not modify in place"]
    pub fn set(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        match self.0.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => entry.1 = Some(value),
            None => self.0.push((name, Some(value))),
        }
        self
    }

    /// Take an attribute off the element.
    #[must_use = "Attrs::unset returns a new value; it does not modify in place"]
    pub fn unset(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        match self.0.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => entry.1 = None,
            None => self.0.push((name, None)),
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
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
        for (name, value) in &self.0 {
            match value {
                Some(value) => list.set(name, value),
                None => list.unset(name),
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
    /// the tag. Only [`COMPONENT_ATTR`] cares, and only to refuse them.
    spreading: bool,
}

impl AttrList {
    pub fn new() -> Self {
        AttrList {
            entries: Vec::new(),
            spreading: false,
        }
    }

    /// Write an attribute whose value is already known as text. The value is
    /// escaped here; callers pass what the author wrote.
    pub fn set(&mut self, name: &str, value: &str) {
        let mut escaped = String::with_capacity(value.len());
        escape_attr_into(value, &mut escaped);
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
