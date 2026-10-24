//! Compiles the client code a Rahti template carries, ahead of time.
//!
//! The browser runtime never turns page text into code: it runs only what the
//! page's client registry holds, which is why a Rahti page needs no
//! `'unsafe-eval'` and why markup injected into a page stays text. `html!`
//! calls this crate for every piece of client code it sees — a `{binding}`, an
//! inline handler, a component `<script>` — and registers the function this
//! crate returns under the exact source text the runtime will read back off
//! the page.
//!
//! Every function here parses its input with a real JavaScript parser, so a
//! binding that is not JavaScript is a compile error on the template rather
//! than a broken page.
//!
//! The functions follow the runtime's registry contract:
//!
//! - an expression is `(scope) => value`;
//! - a handler is `function (scope, event)`, called with the element as
//!   `this`; an expression's function value is then called with the event;
//! - a component script is `function (pp, __pp_server)`, returning `pp` and
//!   the script's top-level bindings, where `__pp_server[i]` is the value of
//!   the script's `i`-th `@{…}`.
//!
//! [`rpc_calls`] reads the `pp.rpc` calls a piece of code makes, which `html!`
//! checks against the `#[rpc]` each one names.
//!
//! An identifier the code does not declare is read from the scope when the
//! scope has it and from the global object otherwise, as a `with` statement
//! would — `__pp_read`, defined once by the registry module, does that.

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, Program, Statement};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

mod rpc;

pub use rpc::{Code, Key, Literal, Payload, RpcCall, RpcName, rpc_calls};

/// Why a piece of client code could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Names a handler reads as its event, unless the scope defines them.
const EVENT_ALIASES: [(&str, &str); 5] = [
    ("e", "event"),
    ("$event", "event"),
    ("target", "event?.target"),
    ("currentTarget", "event?.currentTarget"),
    ("el", "event?.currentTarget"),
];

/// Names strict code cannot declare, or that are never a scope's to supply.
const UNDECLARABLE: [&str; 4] = ["arguments", "eval", "undefined", "globalThis"];

/// The registry function for a `{binding}` whose text is `source`.
pub fn expression(source: &str) -> Result<String, Error> {
    let free = expression_free_names(source)?;
    Ok(format!(
        "(__pp_scope) => {{{} return (\n{source}\n); }}",
        declare_from_scope(&free)
    ))
}

/// The registry function for an inline handler whose text is `source`: an
/// expression, or a statement body.
pub fn handler(source: &str) -> Result<String, Error> {
    let (free, is_expression) = match expression_free_names(source) {
        Ok(free) => (free, true),
        Err(expression_error) => match statements_free_names(source) {
            Ok(free) => (free, false),
            // Report the expression's error: a handler is usually one.
            Err(_) => return Err(expression_error),
        },
    };

    let mut prelude = String::from(" const event = __pp_event;");
    let mut rest = Vec::new();
    for name in free {
        if name == "event" {
            continue;
        }
        match EVENT_ALIASES.iter().find(|(alias, _)| *alias == name) {
            Some((alias, value)) => prelude.push_str(&format!(
                " let {alias} = {key} in __pp_scope ? __pp_scope[{key}] : {value};",
                key = js_string(alias),
            )),
            None => rest.push(name),
        }
    }
    prelude.push_str(&declare_from_scope(&rest));

    let body = if is_expression {
        format!("return (\n{source}\n);")
    } else {
        format!("{{\n{source}\n}}")
    };
    Ok(format!(
        "function (__pp_scope, __pp_event) {{{prelude} {body} }}"
    ))
}

/// A compiled component script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The registry function.
    pub js: String,
    /// The script's top-level bindings, in source order, without `pp`.
    pub bindings: Vec<String>,
}

/// The registry function for a component script whose body is `source`, in
/// which each server value is already written as `__pp_server[i]`.
pub fn component_script(source: &str) -> Result<Script, Error> {
    let wrapped = format!("function __pp_script(pp, __pp_server) {{\n{source}\n}}");
    let mut bindings = Vec::new();
    analyze(&wrapped, |program| {
        let [Statement::FunctionDeclaration(function)] = program.body.as_slice() else {
            return Err(not_one("a component script", "a script body"));
        };
        if function.span.end as usize != wrapped.len() {
            return Err(not_one("a component script", "a script body"));
        }

        let statements = function
            .body
            .as_ref()
            .map(|body| body.statements.as_slice());
        for statement in statements.unwrap_or_default() {
            match statement {
                Statement::VariableDeclaration(declaration) => {
                    for declarator in &declaration.declarations {
                        for identifier in declarator.id.get_binding_identifiers() {
                            bindings.push(identifier.name.to_string());
                        }
                    }
                }
                Statement::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id {
                        bindings.push(id.name.to_string());
                    }
                }
                Statement::ClassDeclaration(class) => {
                    if let Some(id) = &class.id {
                        bindings.push(id.name.to_string());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    bindings.retain(|name| name != "pp");
    bindings.dedup();

    let mut returned = vec!["pp".to_string()];
    returned.extend(bindings.iter().cloned());
    Ok(Script {
        js: format!(
            "function (pp, __pp_server) {{\n{source}\nreturn {{ {} }};\n}}",
            returned.join(", ")
        ),
        bindings,
    })
}

/// Whether `source` is one JavaScript expression.
pub fn is_expression(source: &str) -> bool {
    expression_free_names(source).is_ok()
}

fn expression_free_names(source: &str) -> Result<Vec<String>, Error> {
    if source.trim().is_empty() {
        return Err(Error {
            message: "an empty binding is not an expression".into(),
        });
    }
    // The newline keeps a trailing `// comment` from swallowing the paren.
    let wrapped = format!("({source}\n);");
    analyze(&wrapped, |program| {
        // One statement, whose expression is the parenthesized source itself:
        // `a) + (b` parses too, as a sum, and must not pass as one expression.
        let [Statement::ExpressionStatement(statement)] = program.body.as_slice() else {
            return Err(not_one("a binding", "one JavaScript expression"));
        };
        let Expression::ParenthesizedExpression(parenthesized) = &statement.expression else {
            return Err(not_one("a binding", "one JavaScript expression"));
        };
        if parenthesized.span.start != 0 || parenthesized.span.end as usize != wrapped.len() - 1 {
            return Err(not_one("a binding", "one JavaScript expression"));
        }
        Ok(())
    })
}

fn statements_free_names(source: &str) -> Result<Vec<String>, Error> {
    let wrapped = format!("function __pp_handler() {{\n{source}\n}}");
    analyze(&wrapped, |program| match program.body.as_slice() {
        [Statement::FunctionDeclaration(function)]
            if function.span.end as usize == wrapped.len() =>
        {
            Ok(())
        }
        _ => Err(not_one("a handler", "an expression or a statement body")),
    })
}

/// Parses `text` as strict code, hands the program to `check`, and returns
/// the identifiers it reads without declaring, sorted.
fn analyze(
    text: &str,
    check: impl FnOnce(&Program<'_>) -> Result<(), Error>,
) -> Result<Vec<String>, Error> {
    let allocator = Allocator::default();
    // Registry functions live in a module, so they are strict code.
    let parsed = Parser::new(&allocator, text, SourceType::mjs()).parse();
    if let Some(error) = parsed.diagnostics.errors().next() {
        return Err(Error {
            message: error.message.to_string(),
        });
    }
    let program = parsed.program;
    check(&program)?;
    let built = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    if let Some(error) = built.diagnostics.errors().next() {
        return Err(Error {
            message: error.message.to_string(),
        });
    }
    let mut names: Vec<String> = built
        .semantic
        .scoping()
        .root_unresolved_references()
        .keys()
        .map(|name| name.to_string())
        .filter(|name| !UNDECLARABLE.contains(&name.as_str()))
        .collect();
    names.sort();
    Ok(names)
}

fn declare_from_scope(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!(" let {name} = __pp_read(__pp_scope, {});", js_string(name)))
        .collect()
}

fn not_one(what: &str, expected: &str) -> Error {
    Error {
        message: format!("{what} must be {expected}"),
    }
}

/// `value` as a JavaScript string literal.
pub fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests;
