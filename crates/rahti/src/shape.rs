//! The shape of a server value, as types, for `html!` to check a read against.
//!
//! A script takes server values that are not state:
//!
//! ```ignore
//! const projects = @{Json(&projects)};
//! ```
//!
//! and the template reads fields off them, in bindings and in `pp-for` rows.
//! A macro never sees the type of `Json(&projects)`, so it cannot say whether
//! `project.nmae` is a field. The compiler can.
//!
//! The build reads every struct of the application that a generated module
//! can name, and that module — written once, beside the routes — implements
//! [`Shape`] for each: its JSON as a type built from the ones here, an object
//! being a marker type with one trait per field it has. `html!` then writes
//! each read as trait bounds over the shape of whatever type the server value
//! turns out to have. A field the shape lacks is an unsatisfied bound: an
//! error while type-checking, on the field, with the read's own message.
//!
//! A value whose type has no [`Shape`] — a private struct, a generic
//! parameter, a number — has its reads compile without a word.
//!
//! Nothing here exists at runtime.

use std::marker::PhantomData;

/// The JSON a type serializes to, as a type.
pub trait Shape {
    type Node;
}
impl<T: Shape + ?Sized> Shape for &T {
    type Node = T::Node;
}
impl<T: Shape + serde::Serialize> Shape for crate::Json<T> {
    type Node = T::Node;
}
impl<T: Shape> Shape for Vec<T> {
    type Node = Arr<T::Node>;
}
impl<T: Shape> Shape for [T] {
    type Node = Arr<T::Node>;
}
impl<T: Shape> Shape for Option<T> {
    type Node = Maybe<T::Node>;
}
impl<T: Shape + ?Sized> Shape for Box<T> {
    type Node = T::Node;
}
impl<T: Shape + ?Sized> Shape for std::rc::Rc<T> {
    type Node = T::Node;
}
impl<T: Shape + ?Sized> Shape for std::sync::Arc<T> {
    type Node = T::Node;
}

/// An array whose elements have the shape `T`.
pub struct Arr<T>(PhantomData<T>);

/// The shape `T`, or nothing: an `Option`.
pub struct Maybe<T>(PhantomData<T>);

/// Nothing is known, so every read is allowed: a string, a number, an enum,
/// a map, a type the build could not follow.
pub struct Open;

/// One element: `value[0]`, or a `pp-for` row.
pub trait Element {
    type Out;
}
impl<T> Element for Arr<T> {
    type Out = T;
}
impl<T: Element> Element for Maybe<T> {
    type Out = T::Out;
}
impl Element for Open {
    type Out = Open;
}

/// A field no struct of the application has. Only a shape that answers to
/// any name has it.
pub trait NoField {
    type Out;
}
impl NoField for Open {
    type Out = Open;
}
impl<T> NoField for Arr<T> {
    type Out = Open;
}
impl<T: NoField> NoField for Maybe<T> {
    type Out = T::Out;
}

/// A server value, held to ask what its type is.
pub struct Probe<'v, S: ?Sized>(pub &'v S);

/// A server value whose type has a [`Shape`].
pub struct Known<S: ?Sized>(PhantomData<*const S>);

/// A server value whose type has none: nothing is asked of it.
pub struct Unknown;

/// Reached first by method resolution, and only when the type has a shape.
pub trait HasShape<S: ?Sized> {
    fn shape(&self) -> Known<S> {
        Known(PhantomData)
    }
}
impl<S: Shape + ?Sized> HasShape<S> for Probe<'_, S> {}

/// Every other type, one reference further out.
pub trait NoShape {
    fn shape(&self) -> Unknown {
        Unknown
    }
}
impl<S: ?Sized> NoShape for &Probe<'_, S> {}

/// A read names a field the value has, or the value has no shape.
pub struct Fine;

/// A read names a field the object `M` does not have.
pub struct Missing<M: ?Sized>(PhantomData<*const M>);

impl<M: ?Sized> Missing<M> {
    pub fn new() -> Self {
        Missing(PhantomData)
    }
}

impl<M: ?Sized> Default for Missing<M> {
    fn default() -> Self {
        Self::new()
    }
}
