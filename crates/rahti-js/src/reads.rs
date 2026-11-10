//! What a piece of client code reads off a `pp.rpc` result.
//!
//! `html!` checks each read against the shape the `#[rpc]` returns, so
//! `result.redirct` is a compile error on the template rather than an
//! `undefined` in the browser. This module only finds the reads; the shapes
//! and the checking live with the macro.
//!
//! A read counts when the value it reads is certainly the response:
//!
//! ```text
//! const result = await pp.rpc("save", { name });
//! result.contactId;                 // one read: contactId
//! result.owner.name;                // one read: owner, name
//! const { tags } = await pp.rpc("save", { name });   // one read: tags
//! ```
//!
//! Everything else is left alone, because a wrong "no such field" on correct
//! code is worse than a missed one: a variable that is assigned again, a
//! result that is given fields (`result.extra = …`, `Object.assign(result, …)`),
//! a result handed to `.then`, stored in state, or passed to a function and
//! read there.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentTarget, BindingPattern, CallExpression, Expression, MemberExpression,
    SimpleAssignmentTarget, StaticMemberExpression, UnaryExpression, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder, SymbolId};
use oxc_span::{SourceType, Span};

use crate::Code;

/// One step from a value to a part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `.name`, or a destructured `{ name }`. The span is the name's.
    Field { name: String, span: Range<usize> },
    /// `[0]`: an element.
    Index,
}

/// A chain of steps read off the result of one rpc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The `#[rpc]` whose result is read.
    pub rpc: String,
    pub steps: Vec<Step>,
}

/// Every read of a `pp.rpc` result in `source`, in source order.
pub fn rpc_reads(code: Code, source: &str) -> Vec<Read> {
    if !source.contains("rpc") || !source.contains("await") {
        return Vec::new();
    }
    // Only a script holds an awaited result in a variable: a binding is one
    // expression, and a handler's statements run in a function that is not
    // async.
    if code != Code::Script {
        return Vec::new();
    }
    let (prefix, suffix) = ("async function __pp_script(pp, __pp_server) {\n", "\n}");
    let wrapped = format!("{prefix}{source}{suffix}");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if parsed.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let built = SemanticBuilder::new().build(&parsed.program);
    let mut collector = Collector {
        offset: prefix.len(),
        limit: source.len(),
        scoping: built.semantic.scoping(),
        results: HashMap::new(),
        written: HashSet::new(),
        targets: HashSet::new(),
        reads: Vec::new(),
    };
    collector.visit_program(&parsed.program);
    let Collector {
        written, mut reads, ..
    } = collector;
    reads.retain(|(symbol, _)| symbol.is_none_or(|symbol| !written.contains(&symbol)));
    reads.into_iter().map(|(_, read)| read).collect()
}

struct Collector<'s> {
    offset: usize,
    limit: usize,
    scoping: &'s Scoping,
    /// Variables that hold an rpc's result, and which rpc.
    results: HashMap<SymbolId, String>,
    /// Results that are given fields, whose reads then prove nothing.
    written: HashSet<SymbolId>,
    /// Member expressions that are written to rather than read.
    targets: HashSet<Span>,
    /// Each read, with the variable it was read through.
    reads: Vec<(Option<SymbolId>, Read)>,
}

impl Collector<'_> {
    fn range(&self, span: Span) -> Range<usize> {
        let at = |position: u32| {
            (position as usize)
                .saturating_sub(self.offset)
                .min(self.limit)
        };
        at(span.start)..at(span.end)
    }

    /// The result variable an identifier names, if it is one.
    fn result(&self, expression: &Expression<'_>) -> Option<SymbolId> {
        let Expression::Identifier(identifier) = expression.without_parentheses() else {
            return None;
        };
        let reference = identifier.reference_id.get()?;
        let symbol = self.scoping.get_reference(reference).symbol_id()?;
        self.results.contains_key(&symbol).then_some(symbol)
    }

    /// The steps from a result variable to `expression`, innermost first.
    fn chain(&self, expression: &Expression<'_>, steps: &mut Vec<Step>) -> Option<SymbolId> {
        match expression.without_parentheses() {
            Expression::StaticMemberExpression(member) => {
                let symbol = self.chain(&member.object, steps)?;
                steps.push(Step::Field {
                    name: member.property.name.to_string(),
                    span: self.range(member.property.span),
                });
                Some(symbol)
            }
            Expression::ComputedMemberExpression(member)
                if matches!(member.expression, Expression::NumericLiteral(_)) =>
            {
                let symbol = self.chain(&member.object, steps)?;
                steps.push(Step::Index);
                Some(symbol)
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                oxc_ast::ast::ChainElement::StaticMemberExpression(member) => {
                    let symbol = self.chain(&member.object, steps)?;
                    steps.push(Step::Field {
                        name: member.property.name.to_string(),
                        span: self.range(member.property.span),
                    });
                    Some(symbol)
                }
                _ => None,
            },
            other => self.result(other),
        }
    }

    /// Marks the result a member expression is rooted at as written to.
    fn wrote(&mut self, object: &Expression<'_>) {
        let mut steps = Vec::new();
        if let Some(symbol) = self.chain(object, &mut steps) {
            self.written.insert(symbol);
        }
    }
}

/// The rpc a `await pp.rpc("name", …)` initializer calls.
fn awaited_rpc(init: &Expression<'_>) -> Option<String> {
    let Expression::AwaitExpression(awaited) = init.without_parentheses() else {
        return None;
    };
    let Expression::CallExpression(call) = awaited.argument.without_parentheses() else {
        return None;
    };
    if !crate::rpc::is_pp_rpc(&call.callee) {
        return None;
    }
    match call.arguments.first()? {
        Argument::StringLiteral(literal) => Some(literal.value.to_string()),
        Argument::TemplateLiteral(literal) => literal.single_quasi().map(|q| q.to_string()),
        _ => None,
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(rpc) = declarator.init.as_ref().and_then(awaited_rpc) {
            match &declarator.id {
                BindingPattern::BindingIdentifier(binding) => {
                    if let Some(symbol) = binding.symbol_id.get()
                        && !self.scoping.symbol_is_mutated(symbol)
                    {
                        self.results.insert(symbol, rpc);
                    }
                }
                BindingPattern::ObjectPattern(pattern) => {
                    for property in &pattern.properties {
                        if property.computed {
                            continue;
                        }
                        if let Some(name) = property.key.static_name() {
                            self.reads.push((
                                None,
                                Read {
                                    rpc: rpc.clone(),
                                    steps: vec![Step::Field {
                                        name: name.to_string(),
                                        span: self.range(oxc_span::GetSpan::span(&property.key)),
                                    }],
                                },
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        // `result.extra = …`, `result.count++`.
        if let Some(member) = target.as_member_expression() {
            self.targets.insert(oxc_span::GetSpan::span(member));
            self.wrote(member.object());
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
        walk::walk_assignment_target(self, target);
    }

    fn visit_unary_expression(&mut self, unary: &UnaryExpression<'a>) {
        // `delete result.field`.
        if unary.operator.as_str() == "delete"
            && let Some(member) = unary.argument.without_parentheses().as_member_expression()
        {
            self.targets.insert(oxc_span::GetSpan::span(member));
            self.wrote(member.object());
        }
        walk::walk_unary_expression(self, unary);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        // `Object.assign(result, …)` and its relatives give the result fields.
        if let Expression::StaticMemberExpression(callee) = &call.callee
            && matches!(&callee.object, Expression::Identifier(id) if id.name == "Object")
            && let Some(Argument::Identifier(first)) = call.arguments.first()
            && let Some(reference) = first.reference_id.get()
            && let Some(symbol) = self.scoping.get_reference(reference).symbol_id()
            && self.results.contains_key(&symbol)
        {
            self.written.insert(symbol);
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if !self.targets.contains(&member.span) {
            let mut steps = Vec::new();
            if let Some(symbol) = self.chain(&member.object, &mut steps) {
                steps.push(Step::Field {
                    name: member.property.name.to_string(),
                    span: self.range(member.property.span),
                });
                let rpc = self.results[&symbol].clone();
                self.reads.push((Some(symbol), Read { rpc, steps }));
            }
        }
        walk::walk_static_member_expression(self, member);
    }

    fn visit_member_expression(&mut self, member: &MemberExpression<'a>) {
        walk::walk_member_expression(self, member);
    }
}
