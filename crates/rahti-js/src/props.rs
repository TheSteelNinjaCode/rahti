//! The props a piece of client code reads.
//!
//! A component reads what its caller passed as `pp.props.name`. `html!`
//! checks those names two ways: a component whose root element cannot be
//! given a prop reads one that is never there, and a caller that passes a
//! `{…}` prop to a component whose code never reads it has most likely
//! misspelled it. This module finds the reads; the checking lives with the
//! macro.
//!
//! ```text
//! pp.props.open                 // reads `open`
//! pp.props?.current ?? ""       // reads `current`
//! const { name } = pp.props;    // reads the whole object: any prop
//! ```

use std::collections::HashSet;
use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, StaticMemberExpression};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{SourceType, Span};

use crate::Code;

/// What a piece of client code reads off `pp.props`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropReads {
    /// Each `pp.props.name`, with where the name is written.
    pub names: Vec<(String, Range<usize>)>,
    /// `pp.props` is used whole — destructured, passed on, indexed — so any
    /// prop may be read.
    pub whole: bool,
}

/// What `source` reads off `pp.props`. Nothing, for code that does not
/// parse: compiling it reports that.
pub fn prop_reads(code: Code, source: &str) -> PropReads {
    if !source.contains("props") {
        return PropReads::default();
    }
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
        return PropReads::default();
    }
    let mut reader = Reader {
        offset: prefix.len(),
        limit: source.len(),
        read_through: HashSet::new(),
        found: PropReads::default(),
    };
    reader.visit_program(&parsed.program);
    reader.found
}

/// Is this written exactly `pp.props`?
fn is_pp_props(expression: &Expression<'_>) -> Option<Span> {
    let Expression::StaticMemberExpression(member) = expression.without_parentheses() else {
        return None;
    };
    (member.property.name == "props"
        && matches!(&member.object, Expression::Identifier(id) if id.name == "pp"))
    .then_some(member.span)
}

struct Reader {
    offset: usize,
    limit: usize,
    /// Each `pp.props` that a `.name` is read off.
    read_through: HashSet<Span>,
    found: PropReads,
}

impl<'a> Visit<'a> for Reader {
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Some(props) = is_pp_props(&member.object) {
            let at = |position: u32| {
                (position as usize)
                    .saturating_sub(self.offset)
                    .min(self.limit)
            };
            self.read_through.insert(props);
            self.found.names.push((
                member.property.name.to_string(),
                at(member.property.span.start)..at(member.property.span.end),
            ));
        } else if member.property.name == "props"
            && matches!(&member.object, Expression::Identifier(id) if id.name == "pp")
            && !self.read_through.contains(&member.span)
        {
            self.found.whole = true;
        }
        walk::walk_static_member_expression(self, member);
    }
}
