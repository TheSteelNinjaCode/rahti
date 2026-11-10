//! The `pp.rpc` calls a piece of client code makes.
//!
//! `html!` checks each one against the `#[rpc]` it names, so a misspelled
//! name or argument key is a compile error on the template rather than a 404
//! or a 400 from the server. This module only reads the calls; what they are
//! checked against lives with the macro.
//!
//! A payload is usually an object literal, whose keys are all there to read.
//! One built elsewhere in the same script is followed back to its literal:
//!
//! ```text
//! function ids() { return { project_id: projectId, service_id: serviceId }; }
//! const filter = { status: "open" };
//!
//! pp.rpc("list_events", { ...ids(), ...filter, page });   // five keys, all known
//! pp.rpc("load_service", ids());                          // two keys
//! ```
//!
//! A `const` is followed when nothing but payloads, spreads and field reads
//! ever name it; a function when it is called with no arguments and ends in
//! its one `return`. A key that arrives this way is marked [`Key::followed`]:
//! it counts as sent, but it is not held against the rpc as a key written at
//! the call would be, because one helper often serves several rpcs.
//!
//! A call counts when its callee is written exactly `pp.rpc`. `pp["rpc"]`,
//! `(0, pp.rpc)` and an alias are other ways to reach the same function, and
//! are left alone: the check is a developer aid for the form the conventions
//! teach, and the server validates every payload either way.

use std::ops::Range;

use oxc_allocator::Allocator;
use std::collections::HashSet;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    Argument, ArrowFunctionBody, ArrowFunctionExpression, CallExpression, Expression, Function,
    FunctionBody, IdentifierReference, ObjectExpression, ObjectPropertyKind, ReturnStatement,
    SimpleAssignmentTarget, Statement, StaticMemberExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, Scoping, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;

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
    /// An object literal, or a value followed back to one. `open` when a
    /// spread that could not be followed, or a computed key, means the keys
    /// known are not all the keys sent.
    Object {
        keys: Vec<Key>,
        open: bool,
        span: Range<usize>,
    },
    /// Something whose keys are only known at runtime: `FormData`,
    /// `Object.fromEntries(…)`, a variable or a call that is not followed.
    Unchecked,
}

/// A key written in a payload object literal, shorthand or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub span: Range<usize>,
    pub value: Literal,
    pub value_span: Range<usize>,
    /// Written in an object the payload spreads or is, rather than in the
    /// call itself.
    pub followed: bool,
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
    let built = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    let scoping = built.semantic.scoping();
    // Objects that something other than a payload may have changed.
    let mut uses = Uses {
        scoping,
        allowed: HashSet::new(),
        touched: HashSet::new(),
    };
    uses.visit_program(&parsed.program);
    let mut collector = Collector {
        offset: prefix.len(),
        limit: source.len(),
        scoping,
        nodes: built.semantic.nodes(),
        touched: uses.touched,
        calls: Vec::new(),
    };
    collector.visit_program(&parsed.program);
    collector.calls
}

/// Finds the variables that are named in any way that could change what they
/// hold: handed to a function, aliased, given a field. A payload, a spread
/// and a field read change nothing.
struct Uses<'s> {
    scoping: &'s Scoping,
    /// Identifiers that are a payload, a spread, or read through.
    allowed: HashSet<Span>,
    touched: HashSet<SymbolId>,
}

impl Uses<'_> {
    fn allow(&mut self, expression: &Expression<'_>) {
        if let Expression::Identifier(identifier) = expression.without_parentheses() {
            self.allowed.insert(identifier.span);
        }
    }
}

impl<'a> Visit<'a> for Uses<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if is_pp_rpc(&call.callee)
            && let Some(payload) = call.arguments.get(1).and_then(Argument::as_expression)
        {
            self.allow(payload);
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_object_expression(&mut self, object: &ObjectExpression<'a>) {
        for property in &object.properties {
            if let ObjectPropertyKind::SpreadProperty(spread) = property {
                self.allow(&spread.argument);
            }
        }
        walk::walk_object_expression(self, object);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        // `filter.status = …`: the object is changed.
        if let Some(member) = target.as_member_expression()
            && let Expression::Identifier(identifier) = member.object().without_parentheses()
            && let Some(symbol) = identifier
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
        {
            self.touched.insert(symbol);
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        self.allow(&member.object);
        walk::walk_static_member_expression(self, member);
    }

    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        if !self.allowed.contains(&identifier.span)
            && let Some(symbol) = identifier
                .reference_id
                .get()
                .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
        {
            self.touched.insert(symbol);
        }
    }
}

/// Counts a function's own `return`s, and not those of functions inside it.
struct OwnReturns(usize);

impl<'a> Visit<'a> for OwnReturns {
    fn visit_return_statement(&mut self, _: &ReturnStatement<'a>) {
        self.0 += 1;
    }

    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}

/// What a function called with no arguments returns: the value of its one
/// `return`, which is its last statement.
fn returned<'a>(body: &'a FunctionBody<'a>) -> Option<&'a Expression<'a>> {
    let mut returns = OwnReturns(0);
    returns.visit_function_body(body);
    if returns.0 != 1 {
        return None;
    }
    match body.statements.last()? {
        Statement::ReturnStatement(statement) => statement.argument.as_ref(),
        _ => None,
    }
}

/// How far a payload is followed through `const`s, functions and spreads.
const MAX_DEPTH: usize = 12;

struct Collector<'s, 'a> {
    /// Where the source starts inside the wrapped text.
    offset: usize,
    limit: usize,
    scoping: &'s Scoping,
    nodes: &'s AstNodes<'a>,
    /// Variables whose object something may have changed.
    touched: HashSet<SymbolId>,
    calls: Vec<RpcCall>,
}

impl<'a> Collector<'_, 'a> {
    /// The object literal a value is, when that is certain: the literal
    /// itself, a `const` nothing else touches, or what a function of this
    /// code returns when called with no arguments.
    fn object(&self, value: &'a Expression<'a>, depth: usize) -> Option<&'a ObjectExpression<'a>> {
        if depth > MAX_DEPTH {
            return None;
        }
        match value.without_parentheses() {
            Expression::ObjectExpression(object) => Some(object),
            Expression::Identifier(identifier) => {
                let symbol = self.symbol(identifier)?;
                if self.touched.contains(&symbol)
                    || !self.scoping.symbol_flags(symbol).is_const_variable()
                {
                    return None;
                }
                let AstKind::VariableDeclarator(declarator) =
                    self.nodes.kind(self.scoping.symbol_declaration(symbol))
                else {
                    return None;
                };
                self.object(declarator.init.as_ref()?, depth + 1)
            }
            Expression::CallExpression(call) if call.arguments.is_empty() => {
                let Expression::Identifier(callee) = call.callee.without_parentheses() else {
                    return None;
                };
                let symbol = self.symbol(callee)?;
                if self.scoping.symbol_is_mutated(symbol) {
                    return None;
                }
                let returned = match self.nodes.kind(self.scoping.symbol_declaration(symbol)) {
                    AstKind::Function(function) if !function.r#async && !function.generator => {
                        returned(function.body.as_ref()?)?
                    }
                    AstKind::VariableDeclarator(declarator)
                        if self.scoping.symbol_flags(symbol).is_const_variable() =>
                    {
                        match declarator.init.as_ref()?.without_parentheses() {
                            Expression::ArrowFunctionExpression(arrow) if !arrow.r#async => {
                                match &arrow.body {
                                    ArrowFunctionBody::FunctionBody(body) => returned(body)?,
                                    value => value.as_expression()?,
                                }
                            }
                            Expression::FunctionExpression(function)
                                if !function.r#async && !function.generator =>
                            {
                                returned(function.body.as_ref()?)?
                            }
                            _ => return None,
                        }
                    }
                    _ => return None,
                };
                self.object(returned, depth + 1)
            }
            _ => None,
        }
    }

    fn symbol(&self, identifier: &IdentifierReference<'_>) -> Option<SymbolId> {
        self.scoping
            .get_reference(identifier.reference_id.get()?)
            .symbol_id()
    }

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

    fn payload(&self, call: &'a CallExpression<'a>) -> Payload {
        let Some(argument) = call.arguments.get(1) else {
            return Payload::Missing;
        };
        let Some(value) = argument.as_expression() else {
            return Payload::Unchecked;
        };
        let written = matches!(value.without_parentheses(), Expression::ObjectExpression(_));
        let Some(object) = self.object(value, 0) else {
            return Payload::Unchecked;
        };
        let mut keys = Vec::new();
        let mut open = false;
        self.keys(object, !written, 0, &mut keys, &mut open);
        Payload::Object {
            keys,
            open,
            // A payload built elsewhere is still sent from the call.
            span: self.range(argument.span()),
        }
    }

    /// The keys of `object`, in the order that decides which value wins, with
    /// each spread that can be followed replaced by the keys it brings.
    fn keys(
        &self,
        object: &'a ObjectExpression<'a>,
        followed: bool,
        depth: usize,
        keys: &mut Vec<Key>,
        open: &mut bool,
    ) {
        for property in &object.properties {
            match property {
                ObjectPropertyKind::SpreadProperty(spread) => {
                    match self.object(&spread.argument, depth + 1) {
                        Some(spread) => self.keys(spread, true, depth + 1, keys, open),
                        None => *open = true,
                    }
                }
                ObjectPropertyKind::ObjectProperty(property) if property.computed => *open = true,
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
                            followed,
                        });
                    }
                    None => *open = true,
                },
            }
        }
    }
}

impl<'a> Visit<'a> for Collector<'_, 'a> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if is_pp_rpc(&call.callee) {
            // The node is the tree's own, so what a payload is followed to
            // can be read for as long as the tree is.
            let node = self.nodes.iter().find_map(|node| match node.kind() {
                AstKind::CallExpression(node) if node.span == call.span => Some(node),
                _ => None,
            });
            let payload = match node {
                Some(call) => self.payload(call),
                None => Payload::Unchecked,
            };
            self.calls.push(RpcCall {
                name: self.name(call),
                payload,
            });
        }
        walk::walk_call_expression(self, call);
    }
}

/// `pp.rpc`, written as a static member of the identifier `pp`.
pub(crate) fn is_pp_rpc(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::StaticMemberExpression(member) => {
            member.property.name == "rpc"
                && matches!(&member.object, Expression::Identifier(id) if id.name == "pp")
        }
        _ => false,
    }
}
