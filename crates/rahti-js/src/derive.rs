//! Where the value of an expression comes from, when it comes from another
//! value without changing shape.
//!
//! A list on the page is rarely the state itself. It is the state filtered,
//! or a part of it, or nothing yet:
//!
//! ```text
//! function shown() {
//!     const all = domains || [];
//!     if (!query) return all;
//!     return all.filter((domain) => domain.name.includes(query));
//! }
//! ```
//!
//! Whatever `shown()` returns has the shape `domains` has. This module says
//! so as a [`Derived`]: the names an expression reads and the steps it takes
//! from them, kept only while each step leaves the shape certain — a field,
//! an element, an array method that returns the same elements, a choice
//! between two such values, a fallback to an empty one. Anything else — a
//! `map`, a literal, a function called with arguments — is [`Derived::Unknown`],
//! and the macro then knows nothing about the value.

use std::collections::HashMap;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, CallExpression, ChainElement, Expression, Function,
    FunctionBody, ReturnStatement, Statement, VariableDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder, SymbolId};
use oxc_span::SourceType;
use oxc_syntax::scope::ScopeFlags;

use crate::Step;
use crate::helpers::is_empty;

/// Where a value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Derived {
    /// A name, and the fields read off it: `detail.variables`.
    Name { name: String, steps: Vec<Step> },
    /// What the script's function of this name returns, called with no
    /// arguments.
    Call(String),
    /// `[]`, `{}`, `null`, `undefined`, `""`: no shape, and no contradiction.
    Empty,
    /// One of these: the branches of `a ? b : c`, the sides of `a || b`, the
    /// `return`s of a function.
    Either(Vec<Derived>),
    /// An array with the same elements: `list.filter(…)`, `.slice(…)`,
    /// `.sort(…)`, `.toSorted(…)`, `.reverse()`, `.toReversed()`.
    SameArray(Box<Derived>),
    /// One element of an array, or nothing: `list.find(…)`, `.findLast(…)`,
    /// `.at(…)`.
    Element(Box<Derived>),
    /// Fields read off a value that is not a plain name: `preview().ticks`.
    Path(Box<Derived>, Vec<Step>),
    /// Anything else.
    Unknown,
}

/// A top-level binding of a script whose value is followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    /// A function, whose value is what a call returns; otherwise a `const`.
    pub function: bool,
    pub value: Derived,
}

/// Array methods that return an array of the same elements.
const SAME_ARRAY: [&str; 6] = [
    "filter",
    "slice",
    "sort",
    "toSorted",
    "reverse",
    "toReversed",
];

/// Array methods that return one element, or nothing.
const ELEMENT: [&str; 3] = ["find", "findLast", "at"];

/// Where the value of a binding's expression comes from. Every name it reads
/// is one the code does not declare, which the runtime reads from the scope.
pub fn derive(source: &str) -> Derived {
    let wrapped = format!("({source}\n);");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if parsed.diagnostics.errors().next().is_some() {
        return Derived::Unknown;
    }
    let built = SemanticBuilder::new().build(&parsed.program);
    let [Statement::ExpressionStatement(statement)] = parsed.program.body.as_slice() else {
        return Derived::Unknown;
    };
    let lowering = Lowering {
        scoping: built.semantic.scoping(),
        script: None,
        locals: HashMap::new(),
    };
    lowering.lower(&statement.expression, 0)
}

/// The top-level bindings of a script whose values are followed: each
/// `const` with a value, and each function that is not `async` or a
/// generator. `states` names the script's states by symbol.
pub(crate) fn bindings<'a>(
    body: &'a FunctionBody<'a>,
    scoping: &Scoping,
    states: &HashMap<SymbolId, String>,
) -> Vec<Binding> {
    // The names a function may read by name: the script's own top level.
    let mut top = HashMap::new();
    for statement in &body.statements {
        match statement {
            Statement::VariableDeclaration(declaration) if declaration.kind.is_const() => {
                for declarator in &declaration.declarations {
                    if let BindingPattern::BindingIdentifier(binding) = &declarator.id
                        && let Some(symbol) = binding.symbol_id.get()
                        && declarator.init.is_some()
                    {
                        let function = matches!(
                            declarator
                                .init
                                .as_ref()
                                .map(Expression::without_parentheses),
                            Some(
                                Expression::ArrowFunctionExpression(_)
                                    | Expression::FunctionExpression(_)
                            )
                        );
                        top.insert(symbol, (binding.name.to_string(), function));
                    }
                }
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(id) = &function.id
                    && let Some(symbol) = id.symbol_id.get()
                    && !scoping.symbol_is_mutated(symbol)
                {
                    top.insert(symbol, (id.name.to_string(), true));
                }
            }
            _ => {}
        }
    }
    let script = Script { states, top: &top };

    let mut out = Vec::new();
    let mut push = |name: &str, function: bool, value: Derived| {
        if value != Derived::Unknown {
            out.push(Binding {
                name: name.to_string(),
                function,
                value,
            });
        }
    };
    for statement in &body.statements {
        match statement {
            Statement::VariableDeclaration(declaration) if declaration.kind.is_const() => {
                for declarator in &declaration.declarations {
                    let (BindingPattern::BindingIdentifier(binding), Some(init)) =
                        (&declarator.id, &declarator.init)
                    else {
                        continue;
                    };
                    // A state is followed as a state, not as what it starts from.
                    if binding
                        .symbol_id
                        .get()
                        .is_some_and(|symbol| states.contains_key(&symbol))
                    {
                        continue;
                    }
                    match init.without_parentheses() {
                        Expression::ArrowFunctionExpression(arrow) if !arrow.r#async => {
                            let value = match &arrow.body {
                                oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => {
                                    returned(body, scoping, &script)
                                }
                                value => match value.as_expression() {
                                    Some(value) => Lowering {
                                        scoping,
                                        script: Some(&script),
                                        locals: HashMap::new(),
                                    }
                                    .lower(value, 0),
                                    None => Derived::Unknown,
                                },
                            };
                            push(&binding.name, true, value);
                        }
                        Expression::ArrowFunctionExpression(_) => {}
                        Expression::FunctionExpression(function) => {
                            if let Some(body) = &function.body
                                && !function.r#async
                                && !function.generator
                            {
                                push(&binding.name, true, returned(body, scoping, &script));
                            }
                        }
                        value => {
                            let lowering = Lowering {
                                scoping,
                                script: Some(&script),
                                locals: HashMap::new(),
                            };
                            push(&binding.name, false, lowering.lower(value, 0));
                        }
                    }
                }
            }
            Statement::FunctionDeclaration(function) => {
                if let (Some(id), Some(body)) = (&function.id, &function.body)
                    && !function.r#async
                    && !function.generator
                    && id
                        .symbol_id
                        .get()
                        .is_some_and(|symbol| top.contains_key(&symbol))
                {
                    push(&id.name, true, returned(body, scoping, &script));
                }
            }
            _ => {}
        }
    }
    out
}

/// What a function returns: one of its `return`s.
fn returned(body: &FunctionBody<'_>, scoping: &Scoping, script: &Script<'_>) -> Derived {
    let mut reader = Reader {
        lowering: Lowering {
            scoping,
            script: Some(script),
            locals: HashMap::new(),
        },
        returns: Vec::new(),
    };
    reader.visit_function_body(body);
    let mut values = reader.returns;
    if values.is_empty() || values.contains(&Derived::Unknown) {
        return Derived::Unknown;
    }
    match values.len() {
        1 => values.remove(0),
        _ => Derived::Either(values),
    }
}

/// What a script's own names are.
struct Script<'s> {
    /// The script's states.
    states: &'s HashMap<SymbolId, String>,
    /// Its top-level `const`s and functions, and whether each is a function.
    top: &'s HashMap<SymbolId, (String, bool)>,
}

/// Reads one function's `const`s and `return`s in source order, and not
/// those of the functions inside it.
struct Reader<'s> {
    lowering: Lowering<'s>,
    returns: Vec<Derived>,
}

impl<'a> Visit<'a> for Reader<'_> {
    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        if declaration.kind.is_const() {
            for declarator in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(binding) = &declarator.id
                    && let Some(symbol) = binding.symbol_id.get()
                    && let Some(init) = &declarator.init
                    && !self.lowering.scoping.symbol_is_mutated(symbol)
                {
                    let value = self.lowering.lower(init, 0);
                    self.lowering.locals.insert(symbol, value);
                }
            }
        }
        walk::walk_variable_declaration(self, declaration);
    }

    fn visit_return_statement(&mut self, statement: &ReturnStatement<'a>) {
        self.returns.push(match &statement.argument {
            // `return;` returns nothing.
            None => Derived::Empty,
            Some(argument) => self.lowering.lower(argument, 0),
        });
    }

    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}

/// How deep a value is followed through `const`s before giving up.
const MAX_DEPTH: usize = 16;

struct Lowering<'s> {
    scoping: &'s Scoping,
    /// The script the expression is written in. Without one, every name the
    /// expression does not declare is read from the scope.
    script: Option<&'s Script<'s>>,
    /// The enclosing function's `const`s, and where each one's value comes
    /// from.
    locals: HashMap<SymbolId, Derived>,
}

impl Lowering<'_> {
    fn lower(&self, expression: &Expression<'_>, depth: usize) -> Derived {
        if depth > MAX_DEPTH {
            return Derived::Unknown;
        }
        let expression = expression.without_parentheses();
        if is_empty(expression) {
            return Derived::Empty;
        }
        match expression {
            Expression::Identifier(identifier) => {
                let symbol = identifier
                    .reference_id
                    .get()
                    .and_then(|reference| self.scoping.get_reference(reference).symbol_id());
                let name = |name: &str| Derived::Name {
                    name: name.to_string(),
                    steps: Vec::new(),
                };
                match (symbol, self.script) {
                    (None, None) => name(&identifier.name),
                    (Some(symbol), Some(script)) => {
                        if let Some(state) = script.states.get(&symbol) {
                            name(state)
                        } else if let Some(value) = self.locals.get(&symbol) {
                            value.clone()
                        } else if let Some((top, false)) = script.top.get(&symbol) {
                            name(top)
                        } else {
                            Derived::Unknown
                        }
                    }
                    _ => Derived::Unknown,
                }
            }
            Expression::StaticMemberExpression(member) => step(
                self.lower(&member.object, depth + 1),
                Step::Field {
                    name: member.property.name.to_string(),
                    span: 0..0,
                },
            ),
            Expression::ComputedMemberExpression(member) => {
                step(self.lower(&member.object, depth + 1), Step::Index)
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::StaticMemberExpression(member) => step(
                    self.lower(&member.object, depth + 1),
                    Step::Field {
                        name: member.property.name.to_string(),
                        span: 0..0,
                    },
                ),
                ChainElement::ComputedMemberExpression(member) => {
                    step(self.lower(&member.object, depth + 1), Step::Index)
                }
                ChainElement::CallExpression(call) => self.call(call, depth),
                _ => Derived::Unknown,
            },
            Expression::CallExpression(call) => self.call(call, depth),
            Expression::ConditionalExpression(conditional) => either(vec![
                self.lower(&conditional.consequent, depth + 1),
                self.lower(&conditional.alternate, depth + 1),
            ]),
            Expression::LogicalExpression(logical)
                if matches!(logical.operator.as_str(), "||" | "??") =>
            {
                either(vec![
                    self.lower(&logical.left, depth + 1),
                    self.lower(&logical.right, depth + 1),
                ])
            }
            _ => Derived::Unknown,
        }
    }

    fn call(&self, call: &CallExpression<'_>, depth: usize) -> Derived {
        match call.callee.without_parentheses() {
            Expression::StaticMemberExpression(callee) => {
                let method = callee.property.name.as_str();
                let inner = |this: &Self| match this.lower(&callee.object, depth + 1) {
                    Derived::Unknown | Derived::Empty => None,
                    value => Some(Box::new(value)),
                };
                if SAME_ARRAY.contains(&method) {
                    inner(self).map_or(Derived::Unknown, Derived::SameArray)
                } else if ELEMENT.contains(&method) {
                    inner(self).map_or(Derived::Unknown, Derived::Element)
                } else {
                    Derived::Unknown
                }
            }
            // A function of the script, called with nothing: what it returns
            // depends on nothing the call writes.
            Expression::Identifier(callee) if call.arguments.is_empty() => {
                let symbol = callee
                    .reference_id
                    .get()
                    .and_then(|reference| self.scoping.get_reference(reference).symbol_id());
                match (symbol, self.script) {
                    (None, None) => Derived::Call(callee.name.to_string()),
                    (Some(symbol), Some(script)) => match script.top.get(&symbol) {
                        Some((name, true)) => Derived::Call(name.clone()),
                        _ => Derived::Unknown,
                    },
                    _ => Derived::Unknown,
                }
            }
            _ => Derived::Unknown,
        }
    }
}

/// `value` with one more step read off it.
fn step(value: Derived, step: Step) -> Derived {
    match value {
        Derived::Unknown | Derived::Empty => Derived::Unknown,
        Derived::Name { name, mut steps } => {
            steps.push(step);
            Derived::Name { name, steps }
        }
        Derived::Path(base, mut steps) => {
            steps.push(step);
            Derived::Path(base, steps)
        }
        other => Derived::Path(Box::new(other), vec![step]),
    }
}

/// One of `values`, or [`Derived::Unknown`] when any of them is.
fn either(values: Vec<Derived>) -> Derived {
    if values.contains(&Derived::Unknown) {
        return Derived::Unknown;
    }
    let mut flat = Vec::new();
    for value in values {
        match value {
            Derived::Either(inner) => flat.extend(inner),
            value => flat.push(value),
        }
    }
    match flat.len() {
        1 => flat.remove(0),
        _ => Derived::Either(flat),
    }
}
