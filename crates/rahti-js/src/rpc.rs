//! The `pp.rpc` calls a piece of client code makes.
//!
//! `html!` checks each one against the `#[rpc]` it names, so a misspelled
//! name or argument key is a compile error on the template rather than a 404
//! or a 400 from the server. This module only reads the calls; what they are
//! checked against lives with the macro.
//!
//! A call counts when its callee is written exactly `pp.rpc`. `pp["rpc"]`,
//! `(0, pp.rpc)` and an alias are other ways to reach the same function, and
//! are left alone: the check is a developer aid for the form the conventions
//! teach, and the server validates every payload either way.

use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Argument, CallExpression, Expression, ObjectPropertyKind};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

/// Which kind of client code a source is, which decides how it parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// A `{binding}`: one expression.
    Expression,
    /// An inline handler: an expression or a statement body.
    Handler,
    /// A component `<script>` body.
    Script,
}

/// One `pp.rpc(name, payload)` call. Every span is a byte range in the
/// source handed to [`rpc_calls`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcCall {
    pub name: RpcName,
    pub payload: Payload,
}

/// The first argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcName {
    /// `"greet"`, `'greet'` or `` `greet` ``.
    Literal { value: String, span: Range<usize> },
    /// Anything else — a variable, a concatenation, a spread.
    NotLiteral { span: Range<usize> },
    /// `pp.rpc()`. `span` is the call.
    Missing { span: Range<usize> },
}

/// The second argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// `pp.rpc("name")`.
    Missing,
    /// An object literal. `open` when a spread or a computed key means the
    /// keys written are not all the keys sent.
    Object {
        keys: Vec<Key>,
        open: bool,
        span: Range<usize>,
    },
    /// Something whose keys are only known at runtime: `FormData`,
    /// `Object.fromEntries(…)`, a variable, a call.
    Unchecked,
}

/// A key written in a payload object literal, shorthand or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub span: Range<usize>,
    pub value: Literal,
    pub value_span: Range<usize>,
}

/// Only syntax that proves a JSON value's shape. Variables and calls remain unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Literal {
    Unknown,
    String,
    Number,
    Boolean,
    Null,
    Array,
    Object,
}

fn literal(value: &Expression<'_>) -> Literal {
    match value {
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => Literal::String,
        Expression::NumericLiteral(value) if value.value.is_finite() => Literal::Number,
        Expression::BooleanLiteral(_) => Literal::Boolean,
        Expression::NullLiteral(_) => Literal::Null,
        Expression::ArrayExpression(_) => Literal::Array,
        Expression::ObjectExpression(object)
            if object.properties.iter().all(|property| match property {
                ObjectPropertyKind::SpreadProperty(_) => false,
                ObjectPropertyKind::ObjectProperty(property) => {
                    !property.computed
                        && property
                            .key
                            .static_name()
                            .is_some_and(|name| name != "toJSON" && name != "__proto__")
                }
            }) =>
        {
            Literal::Object
        }
        Expression::ParenthesizedExpression(value) => literal(&value.expression),
        Expression::UnaryExpression(value)
            if matches!(value.operator.as_str(), "+" | "-")
                && matches!(&value.argument, Expression::NumericLiteral(number) if number.value.is_finite()) =>
        {
            Literal::Number
        }
        _ => Literal::Unknown,
    }
}

/// Every `pp.rpc` call in `source`, in source order, nested functions and
/// arrows included. Code that does not parse has none: compiling it reports
/// that error.
pub fn rpc_calls(code: Code, source: &str) -> Vec<RpcCall> {
    if !source.contains("rpc") {
        return Vec::new();
    }
    let (prefix, suffix) = match code {
        Code::Expression => ("(", "\n);"),
        Code::Handler if crate::is_expression(source) => ("(", "\n);"),
        Code::Handler => ("function __pp_handler() {\n", "\n}"),
        Code::Script => ("function __pp_script(pp, __pp_server) {\n", "\n}"),
    };
    let wrapped = format!("{prefix}{source}{suffix}");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if parsed.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let mut collector = Collector {
        offset: prefix.len(),
        limit: source.len(),
        calls: Vec::new(),
    };
    collector.visit_program(&parsed.program);
    collector.calls
}

struct Collector {
    /// Where the source starts inside the wrapped text.
    offset: usize,
    limit: usize,
    calls: Vec<RpcCall>,
}

impl Collector {
    /// `span` as a range in the source rather than in the wrapped text.
    fn range(&self, span: Span) -> Range<usize> {
        let at = |position: u32| {
            (position as usize)
                .saturating_sub(self.offset)
                .min(self.limit)
        };
        at(span.start)..at(span.end)
    }

    fn name(&self, call: &CallExpression<'_>) -> RpcName {
        let Some(argument) = call.arguments.first() else {
            return RpcName::Missing {
                span: self.range(call.span),
            };
        };
        let span = self.range(argument.span());
        let value = match argument {
            Argument::StringLiteral(literal) => Some(literal.value.to_string()),
            Argument::TemplateLiteral(literal) => literal.single_quasi().map(|q| q.to_string()),
            _ => None,
        };
        match value {
            Some(value) => RpcName::Literal { value, span },
            None => RpcName::NotLiteral { span },
        }
    }

    fn payload(&self, call: &CallExpression<'_>) -> Payload {
        let Some(argument) = call.arguments.get(1) else {
            return Payload::Missing;
        };
        let Argument::ObjectExpression(object) = argument else {
            return Payload::Unchecked;
        };
        let mut keys = Vec::new();
        let mut open = false;
        for property in &object.properties {
            match property {
                ObjectPropertyKind::SpreadProperty(_) => open = true,
                ObjectPropertyKind::ObjectProperty(property) if property.computed => open = true,
                ObjectPropertyKind::ObjectProperty(property) => match property.key.static_name() {
                    Some(name) => {
                        // JSON object literals use the final property with a given name.
                        keys.retain(|key: &Key| key.name != name);
                        keys.push(Key {
                            name: name.to_string(),
                            span: self.range(property.key.span()),
                            value: if property.method
                                || property.kind != oxc_ast::ast::PropertyKind::Init
                            {
                                Literal::Unknown
                            } else {
                                literal(&property.value)
                            },
                            value_span: self.range(property.value.span()),
                        });
                    }
                    None => open = true,
                },
            }
        }
        Payload::Object {
            keys,
            open,
            span: self.range(object.span),
        }
    }
}

impl<'a> Visit<'a> for Collector {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if is_pp_rpc(&call.callee) {
            self.calls.push(RpcCall {
                name: self.name(call),
                payload: self.payload(call),
            });
        }
        walk::walk_call_expression(self, call);
    }
}

/// `pp.rpc`, written as a static member of the identifier `pp`.
fn is_pp_rpc(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::StaticMemberExpression(member) => {
            member.property.name == "rpc"
                && matches!(&member.object, Expression::Identifier(id) if id.name == "pp")
        }
        _ => false,
    }
}
