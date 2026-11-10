//! An rpc awaited through a helper that hands back what its callback returns.
//!
//! A script that makes several calls usually wraps them, so the busy flag and
//! the error message are written once:
//!
//! ```text
//! async function act(request) {
//!     setBusy(true);
//!     try {
//!         return await request();
//!     } catch (error) {
//!         setFailure(error.message);
//!         return null;
//!     } finally {
//!         setBusy(false);
//!     }
//! }
//!
//! const found = await act(() => pp.rpc("list_tables", { id }));
//! ```
//!
//! `found` is the response of `list_tables`, or nothing. This module says so
//! when it is certain: the helper is a function the script declares once,
//! every value it returns is its callback's result or an empty value, and
//! the callback written at the call does nothing but return one `pp.rpc`.
//! A helper that returns anything else — a fallback object, the result
//! changed on the way — is not followed.
//!
//! The same response reaches a `.then` callback written on the call:
//! `pp.rpc("list_tables", { id }).then(setTables)`, or
//! `.then((found) => …)`, where `found` is the response.

use std::collections::HashMap;

use oxc_ast::ast::{
    Argument, ArrowFunctionBody, ArrowFunctionExpression, BindingPattern, CallExpression,
    Expression, FormalParameters, Function, FunctionBody, Program, ReturnStatement, Statement,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};
use oxc_syntax::scope::ScopeFlags;

/// The helpers of one piece of code: each function that returns what one of
/// its parameters returns when called, and which parameter.
#[derive(Default)]
pub(crate) struct Helpers(HashMap<SymbolId, usize>);

impl Helpers {
    pub(crate) fn of(program: &Program<'_>, scoping: &Scoping) -> Self {
        let mut finder = Finder {
            scoping,
            found: HashMap::new(),
        };
        finder.visit_program(program);
        Helpers(finder.found)
    }

    /// The rpc whose response `expression` is: `await pp.rpc("name", …)`, or
    /// `await helper(() => pp.rpc("name", …))`.
    pub(crate) fn awaited_rpc(
        &self,
        scoping: &Scoping,
        expression: &Expression<'_>,
    ) -> Option<String> {
        let Expression::AwaitExpression(awaited) = expression.without_parentheses() else {
            return None;
        };
        self.promised_rpc(scoping, &awaited.argument)
    }

    /// The rpc whose response a `.then` callback is handed, and that
    /// callback: `pp.rpc("name", …).then(callback)`, or the same on a
    /// helper's call. A second argument, the failure's, is left alone.
    pub(crate) fn then<'b, 'a>(
        &self,
        scoping: &Scoping,
        call: &'b CallExpression<'a>,
    ) -> Option<(String, &'b Expression<'a>)> {
        let Expression::StaticMemberExpression(callee) = call.callee.without_parentheses() else {
            return None;
        };
        if callee.property.name != "then" {
            return None;
        }
        let rpc = self.promised_rpc(scoping, &callee.object)?;
        Some((rpc, call.arguments.first()?.as_expression()?))
    }

    /// The rpc whose response `expression` is a promise of: `pp.rpc("name",
    /// …)`, or `helper(() => pp.rpc("name", …))`, not yet awaited.
    fn promised_rpc(&self, scoping: &Scoping, expression: &Expression<'_>) -> Option<String> {
        let Expression::CallExpression(call) = expression.without_parentheses() else {
            return None;
        };
        if crate::rpc::is_pp_rpc(&call.callee) {
            return rpc_name(call.arguments.first()?);
        }
        let Expression::Identifier(callee) = &call.callee else {
            return None;
        };
        let symbol = scoping
            .get_reference(callee.reference_id.get()?)
            .symbol_id()?;
        let callback = call.arguments.get(*self.0.get(&symbol)?)?;
        returned_rpc(callback.as_expression()?)
    }
}

/// The variable a callback takes its first argument as, when it is one
/// plain name that is never assigned again: `(result) => …`.
pub(crate) fn first_parameter(scoping: &Scoping, callback: &Expression<'_>) -> Option<SymbolId> {
    let params = match callback.without_parentheses() {
        Expression::ArrowFunctionExpression(arrow) => &arrow.params,
        Expression::FunctionExpression(function) => &function.params,
        _ => return None,
    };
    let BindingPattern::BindingIdentifier(binding) = &params.items.first()?.pattern else {
        return None;
    };
    binding
        .symbol_id
        .get()
        .filter(|symbol| !scoping.symbol_is_mutated(*symbol))
}

/// The name a `pp.rpc` call is given, when it is a literal.
fn rpc_name(argument: &Argument<'_>) -> Option<String> {
    match argument {
        Argument::StringLiteral(literal) => Some(literal.value.to_string()),
        Argument::TemplateLiteral(literal) => literal.single_quasi().map(|q| q.to_string()),
        _ => None,
    }
}

/// The rpc a callback returns: `() => pp.rpc("name", …)`, with or without
/// `async`, `await` or a body of one `return`.
fn returned_rpc(callback: &Expression<'_>) -> Option<String> {
    let body = match callback.without_parentheses() {
        Expression::ArrowFunctionExpression(arrow) => Body::of(&arrow.body),
        Expression::FunctionExpression(function) => Body::Block(function.body.as_ref()?),
        _ => return None,
    };
    let returned = match body {
        Body::Value(value) => value,
        Body::Block(body) => match body.statements.as_slice() {
            [Statement::ReturnStatement(statement)] => statement.argument.as_ref()?,
            _ => return None,
        },
    };
    let mut returned = returned.without_parentheses();
    if let Expression::AwaitExpression(awaited) = returned {
        returned = awaited.argument.without_parentheses();
    }
    let Expression::CallExpression(call) = returned else {
        return None;
    };
    if !crate::rpc::is_pp_rpc(&call.callee) {
        return None;
    }
    rpc_name(call.arguments.first()?)
}

/// Does this value carry no shape of its own?
pub(crate) fn is_empty(expression: &Expression<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(identifier) => identifier.name == "undefined",
        Expression::ArrayExpression(array) => array.elements.is_empty(),
        Expression::ObjectExpression(object) => object.properties.is_empty(),
        Expression::StringLiteral(literal) => literal.value.is_empty(),
        _ => false,
    }
}

/// What a function runs: statements, or an arrow's one expression.
enum Body<'b, 'a> {
    Block(&'b FunctionBody<'a>),
    Value(&'b Expression<'a>),
}

impl<'b, 'a> Body<'b, 'a> {
    fn of(body: &'b ArrowFunctionBody<'a>) -> Self {
        match body {
            ArrowFunctionBody::FunctionBody(body) => Body::Block(body),
            value => match value.as_expression() {
                Some(value) => Body::Value(value),
                None => unreachable!("an arrow body is statements or an expression"),
            },
        }
    }
}

struct Finder<'s> {
    scoping: &'s Scoping,
    found: HashMap<SymbolId, usize>,
}

impl Finder<'_> {
    /// Records `symbol` as a helper when the function it names returns only
    /// what one of its parameters returns.
    fn consider(
        &mut self,
        symbol: Option<SymbolId>,
        params: &FormalParameters<'_>,
        body: Body<'_, '_>,
    ) {
        let Some(symbol) = symbol else { return };
        // A function that is assigned again is not one function.
        if self.scoping.symbol_is_mutated(symbol) {
            return;
        }
        let parameters: Vec<Option<SymbolId>> = params
            .items
            .iter()
            .map(|parameter| match &parameter.pattern {
                BindingPattern::BindingIdentifier(binding) => binding
                    .symbol_id
                    .get()
                    .filter(|symbol| !self.scoping.symbol_is_mutated(*symbol)),
                _ => None,
            })
            .collect();

        let mut returns = Returns {
            scoping: self.scoping,
            parameters: &parameters,
            through: None,
            other: false,
        };
        match body {
            Body::Value(value) => returns.returned(Some(value)),
            Body::Block(body) => returns.visit_function_body(body),
        }
        if let (Some(index), false) = (returns.through, returns.other) {
            self.found.insert(symbol, index);
        }
    }
}

impl<'a> Visit<'a> for Finder<'_> {
    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        if let (Some(id), Some(body)) = (&function.id, &function.body)
            && !function.generator
        {
            self.consider(id.symbol_id.get(), &function.params, Body::Block(body));
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(binding) = &declarator.id {
            let symbol = binding.symbol_id.get();
            match declarator
                .init
                .as_ref()
                .map(Expression::without_parentheses)
            {
                Some(Expression::ArrowFunctionExpression(arrow)) => {
                    self.consider(symbol, &arrow.params, Body::of(&arrow.body));
                }
                Some(Expression::FunctionExpression(function)) if !function.generator => {
                    if let Some(body) = &function.body {
                        self.consider(symbol, &function.params, Body::Block(body));
                    }
                }
                _ => {}
            }
        }
        walk::walk_variable_declarator(self, declarator);
    }
}

/// Reads the `return`s of one function, and not of the functions inside it.
struct Returns<'s> {
    scoping: &'s Scoping,
    parameters: &'s [Option<SymbolId>],
    /// The parameter whose result is returned.
    through: Option<usize>,
    /// Something else is returned, or two different parameters' results.
    other: bool,
}

impl Returns<'_> {
    fn returned(&mut self, argument: Option<&Expression<'_>>) {
        let Some(argument) = argument else { return };
        let mut value = argument.without_parentheses();
        if is_empty(value) {
            return;
        }
        // `(await request()) ?? null`: the result, or nothing.
        if let Expression::LogicalExpression(logical) = value
            && is_empty(&logical.right)
        {
            value = logical.left.without_parentheses();
        }
        if let Expression::AwaitExpression(awaited) = value {
            value = awaited.argument.without_parentheses();
        }
        let index = match value {
            Expression::CallExpression(call) => match &call.callee {
                Expression::Identifier(callee) => callee
                    .reference_id
                    .get()
                    .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
                    .and_then(|symbol| {
                        self.parameters
                            .iter()
                            .position(|parameter| *parameter == Some(symbol))
                    }),
                _ => None,
            },
            _ => None,
        };
        match (index, self.through) {
            (Some(index), None) => self.through = Some(index),
            (Some(index), Some(through)) if index == through => {}
            _ => self.other = true,
        }
    }
}

impl<'a> Visit<'a> for Returns<'_> {
    fn visit_return_statement(&mut self, statement: &ReturnStatement<'a>) {
        self.returned(statement.argument.as_ref());
    }

    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}
