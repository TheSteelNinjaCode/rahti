//! What a piece of client code does with state.
//!
//! A `pp.state` value has no declared type, but what is put into it often
//! does: the result of a `pp.rpc`, whose shape the `#[rpc]` fixes.
//!
//! ```text
//! const [domains, setDomains] = pp.state([]);
//! const list = await pp.rpc("list_domains", {});
//! setDomains(list);                 // domains holds list_domains' response
//! domains[0].name;                  // a read of that response
//! ```
//!
//! This module reports the facts and decides nothing: each state a script
//! declares, and each use of a name — what a setter is called with, which
//! fields are read off a value, whether a name is used some other way. The
//! macro joins them into a type, or into no type at all when any use leaves
//! room for doubt.
//!
//! In a script the names followed are the states and setters it declares, by
//! symbol, so a parameter that shadows one is not mistaken for it. In a
//! binding or a handler they are the names the code does not declare, which
//! the runtime reads from the scope.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, ChainElement, ComputedMemberExpression, Expression,
    IdentifierReference, SimpleAssignmentTarget, StaticMemberExpression, UnaryExpression,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};

use crate::helpers::{Helpers, is_empty};
use crate::{Code, Step};

/// Where a value put into state comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The response of `rpc`, or the part of it `steps` lead to.
    Rpc { rpc: String, steps: Vec<Step> },
    /// `[]`, `{}`, `null`, `undefined`, `""`: a value that carries no shape
    /// and contradicts none.
    Empty,
    /// A server value: the script's `@{…}` at this position, counted from
    /// zero, when it stands alone.
    Server(Option<usize>),
    /// Anything else, as written.
    Unknown(String),
}

/// One `const [name, setter] = pp.state(initial)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub name: String,
    pub setter: Option<String>,
    pub initial: Vec<Origin>,
}

/// What is done with a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Use {
    /// `name.a.b`, `name[0].a`: fields read off it.
    Read(Vec<Step>),
    /// `name.a = …`, `delete name.a`, `Object.assign(name, …)`: it is given
    /// or loses fields in place.
    Write,
    /// `name(argument)`, with where the first argument comes from. A
    /// conditional argument comes from both of its branches.
    Call(Vec<Origin>),
    /// Named in any other way: handed on, aliased, called through.
    Other,
}

/// One use of one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameUse {
    pub name: String,
    pub what: Use,
}

/// The states a piece of code declares and the uses it makes of names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateUses {
    /// Declared states, in source order. Only a script declares any.
    pub states: Vec<State>,
    /// In a script, uses of its states and setters. In a binding or a
    /// handler, uses of every name the code does not declare.
    pub uses: Vec<NameUse>,
    /// Every field name the code assigns, updates or deletes on any value:
    /// `row.open = true` names `open`. A value reached through an alias or a
    /// callback's parameter can be given such a field in place.
    pub assigned: Vec<String>,
    /// The code gives a value fields this cannot name: `Object.assign(row,
    /// extra)`, `Object.defineProperty(row, key, …)`.
    pub assigns_unnamed: bool,
    /// A script's top-level `const`s and functions whose values come from
    /// other values without changing shape. See [`crate::Derived`].
    pub derived: Vec<crate::Binding>,
    /// A script's top-level `const name = @{…}`: the name, and which of the
    /// script's server values it is, counted from zero. Its uses are among
    /// [`StateUses::uses`], as a state's are.
    pub server: Vec<(String, usize)>,
}

/// The states `source` declares and the uses it makes of names. Empty for
/// code that does not parse: `html!` reports that itself.
pub fn state_uses(code: Code, source: &str) -> StateUses {
    let (prefix, suffix) = match code {
        Code::Script => ("async function __pp_script(pp, __pp_server) {\n", "\n}"),
        Code::Expression => ("(", "\n);"),
        Code::Handler if crate::is_expression(source) => ("(", "\n);"),
        Code::Handler => ("function __pp_handler() {\n", "\n}"),
    };
    let wrapped = format!("{prefix}{source}{suffix}");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if parsed.diagnostics.errors().next().is_some() {
        return StateUses::default();
    }
    let built = SemanticBuilder::new().build(&parsed.program);
    let scoping = built.semantic.scoping();

    // States first: a function declared above a state may use it.
    let mut declared = Declared {
        text: &wrapped,
        states: Vec::new(),
        tracked: HashMap::new(),
    };
    if code == Code::Script {
        declared.visit_program(&parsed.program);
    }
    let Declared {
        states,
        mut tracked,
        ..
    } = declared;
    // The script's server values that are not state: `const rows = @{…}`.
    let mut server = Vec::new();
    if let (Code::Script, [oxc_ast::ast::Statement::FunctionDeclaration(script)]) =
        (code, parsed.program.body.as_slice())
        && let Some(body) = &script.body
    {
        for statement in &body.statements {
            let oxc_ast::ast::Statement::VariableDeclaration(declaration) = statement else {
                continue;
            };
            if !declaration.kind.is_const() {
                continue;
            }
            for declarator in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(binding) = &declarator.id
                    && let Some(symbol) = binding.symbol_id.get()
                    && let Some(Some(index)) = declarator.init.as_ref().and_then(server_value)
                {
                    tracked.insert(symbol, binding.name.to_string());
                    server.push((binding.name.to_string(), index));
                }
            }
        }
    }
    // The script's own lists: what its functions return, by state.
    let derived = match (code, parsed.program.body.as_slice()) {
        (Code::Script, [oxc_ast::ast::Statement::FunctionDeclaration(script)]) => {
            let named: HashMap<SymbolId, String> = states
                .iter()
                .filter_map(|(_, state)| {
                    tracked
                        .iter()
                        .find(|(_, name)| **name == state.name)
                        .map(|(symbol, name)| (*symbol, name.clone()))
                })
                .collect();
            script
                .body
                .as_ref()
                .map(|body| crate::derive::bindings(body, scoping, &named))
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };

    let mut collector = Collector {
        text: &wrapped,
        offset: prefix.len(),
        limit: source.len(),
        scoping,
        helpers: Helpers::of(&parsed.program, scoping),
        free: code != Code::Script,
        tracked,
        results: HashMap::new(),
        written: HashSet::new(),
        handled: HashSet::new(),
        uses: Vec::new(),
        assigned: Vec::new(),
        assigns_unnamed: false,
    };
    collector.visit_program(&parsed.program);
    let Collector {
        written,
        mut uses,
        mut assigned,
        assigns_unnamed,
        ..
    } = collector;
    assigned.sort();
    assigned.dedup();

    // A result that is given fields proves nothing about what it holds.
    let settle = |origins: &mut Vec<(Option<SymbolId>, Origin)>| {
        for (symbol, origin) in origins {
            if symbol.is_some_and(|symbol| written.contains(&symbol)) {
                *origin = Origin::Unknown("a result that is given fields".into());
            }
        }
    };
    let mut out = StateUses {
        assigned,
        assigns_unnamed,
        derived,
        server,
        ..StateUses::default()
    };
    for (mut initial, state) in states {
        settle(&mut initial);
        out.states.push(State {
            initial: initial.into_iter().map(|(_, origin)| origin).collect(),
            ..state
        });
    }
    for (name, what) in &mut uses {
        let what = match what {
            Found::Read(steps) => Use::Read(std::mem::take(steps)),
            Found::Write => Use::Write,
            Found::Other => Use::Other,
            Found::Call(origins) => {
                settle(origins);
                Use::Call(origins.drain(..).map(|(_, origin)| origin).collect())
            }
        };
        out.uses.push(NameUse {
            name: std::mem::take(name),
            what,
        });
    }
    out
}

/// An origin, with the result variable it was read through.
type Traced = (Option<SymbolId>, Origin);

enum Found {
    Read(Vec<Step>),
    Write,
    Call(Vec<Traced>),
    Other,
}

/// Is `callee` written exactly `pp.state`?
fn is_pp_state(callee: &Expression<'_>) -> bool {
    matches!(callee, Expression::StaticMemberExpression(member)
        if member.property.name == "state"
            && matches!(&member.object, Expression::Identifier(id) if id.name == "pp"))
}

/// Finds the `pp.state` declarations of a script.
struct Declared<'s> {
    text: &'s str,
    states: Vec<(Vec<Traced>, State)>,
    /// State and setter symbols, and their names.
    tracked: HashMap<SymbolId, String>,
}

impl<'a> Visit<'a> for Declared<'_> {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let Some(Expression::CallExpression(call)) = &declarator.init
            && is_pp_state(&call.callee)
            && let BindingPattern::ArrayPattern(pattern) = &declarator.id
            && pattern.rest.is_none()
            && let Some(Some(BindingPattern::BindingIdentifier(value))) = pattern.elements.first()
            && let Some(value_symbol) = value.symbol_id.get()
        {
            let setter = match pattern.elements.get(1) {
                Some(Some(BindingPattern::BindingIdentifier(setter))) => {
                    setter.symbol_id.get().map(|symbol| (symbol, &setter.name))
                }
                _ => None,
            };
            // The initial value is written before any result exists, so it
            // is empty, a server value, or something this does not follow.
            let initial = match call.arguments.first() {
                None => vec![(None, Origin::Empty)],
                Some(argument) => match argument.as_expression() {
                    Some(expression) => initial_origins(self.text, expression),
                    None => vec![(None, Origin::Unknown("a spread".into()))],
                },
            };
            self.tracked.insert(value_symbol, value.name.to_string());
            if let Some((symbol, name)) = setter {
                self.tracked.insert(symbol, name.to_string());
            }
            self.states.push((
                initial,
                State {
                    name: value.name.to_string(),
                    setter: setter.map(|(_, name)| name.to_string()),
                    initial: Vec::new(),
                },
            ));
        }
        walk::walk_variable_declarator(self, declarator);
    }
}

/// The `i` of `__pp_server[i]`, which stands for a script's `i`-th `@{…}`.
/// `Some(None)` for a server value read some other way.
fn server_value(expression: &Expression<'_>) -> Option<Option<usize>> {
    let Expression::ComputedMemberExpression(member) = expression.without_parentheses() else {
        return None;
    };
    if !matches!(&member.object, Expression::Identifier(id) if id.name == "__pp_server") {
        return None;
    }
    Some(match &member.expression {
        Expression::NumericLiteral(index) if index.value >= 0.0 && index.value.fract() == 0.0 => {
            Some(index.value as usize)
        }
        _ => None,
    })
}

fn initial_origins(text: &str, expression: &Expression<'_>) -> Vec<Traced> {
    let expression = expression.without_parentheses();
    if is_empty(expression) {
        return vec![(None, Origin::Empty)];
    }
    if let Some(index) = server_value(expression) {
        return vec![(None, Origin::Server(index))];
    }
    if let Expression::LogicalExpression(logical) = expression
        && is_empty(&logical.right)
    {
        return initial_origins(text, &logical.left);
    }
    vec![(None, Origin::Unknown(excerpt(text, expression.span())))]
}

/// A short, one-line copy of the code at `span`, for a report.
fn excerpt(text: &str, span: Span) -> String {
    let code = text
        .get(span.start as usize..span.end as usize)
        .unwrap_or_default();
    let mut line = code.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.len() > 80 {
        let mut end = 77;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

struct Collector<'s> {
    text: &'s str,
    offset: usize,
    limit: usize,
    scoping: &'s Scoping,
    /// The script's functions that hand back what a callback returns.
    helpers: Helpers,
    /// Follow the names the code does not declare, rather than `tracked`.
    free: bool,
    tracked: HashMap<SymbolId, String>,
    /// Variables that hold an rpc's result, and which rpcs.
    results: HashMap<SymbolId, Vec<String>>,
    /// Results that are given fields.
    written: HashSet<SymbolId>,
    /// Identifiers already accounted for by the expression around them.
    handled: HashSet<Span>,
    uses: Vec<(String, Found)>,
    assigned: Vec<String>,
    assigns_unnamed: bool,
}

impl<'a> Collector<'_> {
    fn range(&self, span: Span) -> Range<usize> {
        let at = |position: u32| {
            (position as usize)
                .saturating_sub(self.offset)
                .min(self.limit)
        };
        at(span.start)..at(span.end)
    }

    fn symbol(&self, identifier: &IdentifierReference<'_>) -> Option<SymbolId> {
        let reference = identifier.reference_id.get()?;
        self.scoping.get_reference(reference).symbol_id()
    }

    /// The name this identifier is followed under, when it is followed.
    fn followed(&self, identifier: &IdentifierReference<'_>) -> Option<String> {
        match self.symbol(identifier) {
            Some(symbol) if !self.free => self.tracked.get(&symbol).cloned(),
            None if self.free => Some(identifier.name.to_string()),
            _ => None,
        }
    }

    /// The identifier a chain of member reads starts at, and the steps from
    /// it to `expression`.
    fn chain<'b>(
        &self,
        expression: &'b Expression<'a>,
        steps: &mut Vec<Step>,
    ) -> Option<&'b IdentifierReference<'a>> {
        match expression.without_parentheses() {
            Expression::Identifier(identifier) => Some(identifier),
            Expression::StaticMemberExpression(member) => self.chain_field(member, steps),
            Expression::ComputedMemberExpression(member) => {
                let root = self.chain(&member.object, steps)?;
                steps.push(Step::Index);
                Some(root)
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::StaticMemberExpression(member) => self.chain_field(member, steps),
                ChainElement::ComputedMemberExpression(member) => {
                    let root = self.chain(&member.object, steps)?;
                    steps.push(Step::Index);
                    Some(root)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn chain_field<'b>(
        &self,
        member: &'b StaticMemberExpression<'a>,
        steps: &mut Vec<Step>,
    ) -> Option<&'b IdentifierReference<'a>> {
        let root = self.chain(&member.object, steps)?;
        steps.push(Step::Field {
            name: member.property.name.to_string(),
            span: self.range(member.property.span),
        });
        Some(root)
    }

    /// The rpcs a result variable's initializer awaits: one, or one per
    /// branch of a conditional. `None` when any branch is something else.
    fn awaited_rpcs(&self, expression: &Expression<'a>, out: &mut Vec<String>) -> Option<()> {
        match expression.without_parentheses() {
            Expression::ConditionalExpression(conditional) => {
                self.awaited_rpcs(&conditional.consequent, out)?;
                self.awaited_rpcs(&conditional.alternate, out)
            }
            other => {
                out.push(self.helpers.awaited_rpc(self.scoping, other)?);
                Some(())
            }
        }
    }

    /// Where the value of `expression` comes from.
    fn origins(&self, expression: &Expression<'a>, out: &mut Vec<Traced>) {
        let expression = expression.without_parentheses();
        if is_empty(expression) {
            out.push((None, Origin::Empty));
            return;
        }
        if let Some(rpc) = self.helpers.awaited_rpc(self.scoping, expression) {
            out.push((
                None,
                Origin::Rpc {
                    rpc,
                    steps: Vec::new(),
                },
            ));
            return;
        }
        match expression {
            Expression::ConditionalExpression(conditional) => {
                self.origins(&conditional.consequent, out);
                self.origins(&conditional.alternate, out);
                return;
            }
            // `list ?? []`, `list || []`: the list, or nothing.
            Expression::LogicalExpression(logical) if is_empty(&logical.right) => {
                self.origins(&logical.left, out);
                out.push((None, Origin::Empty));
                return;
            }
            _ => {}
        }
        let mut steps = Vec::new();
        if let Some(root) = self.chain(expression, &mut steps)
            && let Some(symbol) = self.symbol(root)
            && let Some(rpcs) = self.results.get(&symbol)
        {
            for rpc in rpcs {
                out.push((
                    Some(symbol),
                    Origin::Rpc {
                        rpc: rpc.clone(),
                        steps: steps.clone(),
                    },
                ));
            }
            return;
        }
        out.push((None, Origin::Unknown(excerpt(self.text, expression.span()))));
    }

    /// Records that the value a member expression is rooted at is written to.
    fn wrote(&mut self, object: &Expression<'a>) {
        let mut steps = Vec::new();
        let Some(root) = self.chain(object, &mut steps) else {
            return;
        };
        let span = root.span;
        if let Some(symbol) = self.symbol(root)
            && self.results.contains_key(&symbol)
        {
            self.written.insert(symbol);
        }
        if let Some(name) = self.followed(root)
            && self.handled.insert(span)
        {
            self.uses.push((name, Found::Write));
        }
    }

    /// Records the read a member expression makes, once, at its outermost.
    fn read(&mut self, root: Option<&IdentifierReference<'a>>, steps: Vec<Step>) {
        let Some(root) = root else { return };
        if let Some(name) = self.followed(root)
            && self.handled.insert(root.span)
        {
            self.uses.push((name, Found::Read(steps)));
        }
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        let mut rpcs = Vec::new();
        if let Some(init) = &declarator.init
            && self.awaited_rpcs(init, &mut rpcs).is_some()
            && let BindingPattern::BindingIdentifier(binding) = &declarator.id
            && let Some(symbol) = binding.symbol_id.get()
            && !self.scoping.symbol_is_mutated(symbol)
        {
            self.results.insert(symbol, rpcs);
        }
        walk::walk_variable_declarator(self, declarator);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        if let Some(member) = target.as_member_expression() {
            if let Some(name) = member.static_property_name() {
                self.assigned.push(name.to_string());
            }
            self.wrote(member.object());
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_unary_expression(&mut self, unary: &UnaryExpression<'a>) {
        if unary.operator.as_str() == "delete"
            && let Some(member) = unary.argument.without_parentheses().as_member_expression()
        {
            if let Some(name) = member.static_property_name() {
                self.assigned.push(name.to_string());
            }
            self.wrote(member.object());
        }
        walk::walk_unary_expression(self, unary);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if let Some((rpc, callback)) = self.helpers.then(self.scoping, call) {
            match callback.without_parentheses() {
                // `pp.rpc("list", {}).then(setRows)`: the setter is called
                // with the response, as surely as `setRows(await …)`.
                Expression::Identifier(setter) => {
                    if let Some(name) = self.followed(setter)
                        && self.handled.insert(setter.span)
                    {
                        let origin = Origin::Rpc {
                            rpc,
                            steps: Vec::new(),
                        };
                        self.uses.push((name, Found::Call(vec![(None, origin)])));
                    }
                }
                // `.then((rows) => setRows(rows || []))`: `rows` is the
                // response.
                callback => {
                    if let Some(symbol) = crate::helpers::first_parameter(self.scoping, callback) {
                        self.results.insert(symbol, vec![rpc]);
                    }
                }
            }
        }
        // `Object.assign(value, …)` and its relatives give the value fields.
        if let Expression::StaticMemberExpression(callee) = &call.callee
            && matches!(&callee.object, Expression::Identifier(id) if id.name == "Object")
            && let Some(first) = call.arguments.first().and_then(Argument::as_expression)
        {
            // `Object.assign({}, a, b)` builds a new object; `Object.keys`
            // and the other readers give nothing fields.
            let gives = matches!(
                callee.property.name.as_str(),
                "assign" | "defineProperty" | "defineProperties" | "setPrototypeOf"
            );
            if gives && !matches!(first.without_parentheses(), Expression::ObjectExpression(_)) {
                self.assigns_unnamed = true;
            }
            self.wrote(first);
        }
        if let Expression::Identifier(callee) = &call.callee
            && let Some(name) = self.followed(callee)
            && self.handled.insert(callee.span)
        {
            let mut origins = Vec::new();
            match call.arguments.first() {
                None => origins.push((None, Origin::Empty)),
                Some(argument) => match argument.as_expression() {
                    Some(expression) => self.origins(expression, &mut origins),
                    None => origins.push((None, Origin::Unknown("a spread".into()))),
                },
            }
            self.uses.push((name, Found::Call(origins)));
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        let mut steps = Vec::new();
        let root = self.chain_field(member, &mut steps);
        self.read(root, steps);
        walk::walk_static_member_expression(self, member);
    }

    fn visit_computed_member_expression(&mut self, member: &ComputedMemberExpression<'a>) {
        let mut steps = Vec::new();
        let root = self.chain(&member.object, &mut steps);
        steps.push(Step::Index);
        self.read(root, steps);
        walk::walk_computed_member_expression(self, member);
    }

    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        if let Some(name) = self.followed(identifier)
            && self.handled.insert(identifier.span)
        {
            self.uses.push((name, Found::Other));
        }
    }
}
