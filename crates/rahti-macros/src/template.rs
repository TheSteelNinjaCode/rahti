//! A template written as one raw string: `html! {r##"…"##}`.
//!
//! The markup is HTML, read as text. Nothing in it passes through the Rust
//! lexer, so JavaScript, CSS and prose arrive exactly as written: single
//! quotes, template literals, `"#fff"`, comments and apostrophes included.
//! Two dialects stand out from the HTML:
//!
//! - `@{rust_expression}` — a server value, the only Rust in the template;
//! - `{javascript_expression}` — a PulsePoint binding, in text or as a whole
//!   quoted attribute value (`hidden="{!open}"`).
//!
//! This module turns that text into the token shapes the [`crate::parse`]
//! grammar reads, so every rule it enforces — one root, components and
//! props, slots, owner-scoped children, event ownership — holds unchanged.
//! Text becomes one string literal per run; a `<script>` or `<style>` body
//! becomes a raw string literal; a client binding becomes a marked group
//! carrying its JavaScript source verbatim (see [`client_marker`]), since
//! JavaScript cannot be lexed as Rust.
//!
//! The checks that need a position run here, where the position is known, so
//! each refusal names the file, line and column inside the template.

use std::str::FromStr;

use proc_macro2::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

use crate::parse::Error;

#[cfg(test)]
#[path = "tests/template.rs"]
mod tests;

/// The identifier that marks a client binding's group. Nothing an author
/// writes produces it: a template is a string, so every token `parse` reads
/// comes from this module.
pub const CLIENT_MARKER: &str = "__rahti_client";

/// Where the template literal starts, so a position inside it can be named.
#[derive(Clone, Debug)]
pub struct Origin {
    pub file: String,
    /// One-indexed line of the literal's opening `r`.
    pub line: usize,
    /// One-indexed column of the literal's opening `r`.
    pub column: usize,
    /// Length of `r##"`: where the template text starts on the first line.
    pub prefix: usize,
}

/// The contents of a raw string literal, and the length of its `r#…"`.
pub fn raw_template(lit: &Literal) -> Option<(String, usize)> {
    let repr = lit.to_string();
    let hashes = repr
        .strip_prefix('r')?
        .chars()
        .take_while(|c| *c == '#')
        .count();
    let open = 1 + hashes;
    if repr.as_bytes().get(open) != Some(&b'"') {
        return None;
    }
    let body = repr[open + 1..repr.len() - 1 - hashes].to_string();
    Some((body, open + 1))
}

/// A marked group holding a client binding's source, braces included.
pub fn client_marker(source: &str, span: Span) -> TokenTree {
    let mut stream = TokenStream::new();
    stream.extend([
        TokenTree::Ident(Ident::new(CLIENT_MARKER, span)),
        TokenTree::Literal(with_span(Literal::string(source), span)),
    ]);
    let mut group = Group::new(Delimiter::Brace, stream);
    group.set_span(span);
    TokenTree::Group(group)
}

/// The JavaScript a [`client_marker`] group carries, if `group` is one.
pub fn client_source(group: &Group) -> Option<String> {
    let mut tokens = group.stream().into_iter();
    match (tokens.next(), tokens.next(), tokens.next()) {
        (Some(TokenTree::Ident(id)), Some(TokenTree::Literal(lit)), None)
            if id == CLIENT_MARKER =>
        {
            crate::parse::unquote_literal(&lit.to_string())
        }
        _ => None,
    }
}

fn with_span(mut lit: Literal, span: Span) -> Literal {
    lit.set_span(span);
    lit
}

/// Elements whose content is text the browser does not parse as markup.
const RAW_TEXT: [&str; 2] = ["script", "style"];
/// Elements whose whitespace is content.
const PRESERVE_SPACE: [&str; 2] = ["pre", "textarea"];

/// The tokens `crate::parse` reads for this template.
pub fn tokens(source: &str, span: Span, origin: &Origin) -> Result<TokenStream, Error> {
    let mut lexer = Lexer {
        src: source,
        pos: 0,
        span,
        origin,
        out: Vec::new(),
        open: Vec::new(),
        text: String::new(),
    };
    lexer.run()?;
    lexer.flush();
    Ok(lexer.out.into_iter().collect())
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    span: Span,
    origin: &'a Origin,
    out: Vec<TokenTree>,
    /// Open elements by name, for whitespace rules and raw text.
    open: Vec<String>,
    /// Text not yet written. One run of text is one string literal, which
    /// the grammar keeps verbatim, so adjacent pieces are joined first.
    text: String,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    /// `file:line:column` of a byte offset in the template.
    fn at(&self, offset: usize) -> String {
        let before = &self.src[..offset.min(self.src.len())];
        let newlines = before.matches('\n').count();
        let line = self.origin.line + newlines;
        let column = match before.rfind('\n') {
            Some(nl) => before[nl + 1..].chars().count() + 1,
            None => self.origin.column + self.origin.prefix + before.chars().count(),
        };
        format!("{}:{line}:{column}", self.origin.file)
    }

    fn fail<T>(&self, offset: usize, msg: impl AsRef<str>) -> Result<T, Error> {
        let msg = msg.as_ref();
        let msg = msg.strip_prefix("rahti: ").unwrap_or(msg);
        Err(Error {
            span: self.span,
            msg: format!("rahti: {} — {msg}", self.at(offset)),
        })
    }

    /// Write the pending text as one string literal.
    fn flush(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.out.push(TokenTree::Literal(with_span(
                Literal::string(&text),
                self.span,
            )));
        }
    }

    /// Any token but text: the pending text goes first.
    fn push(&mut self, tt: TokenTree) {
        self.flush();
        self.out.push(tt);
    }

    fn punct(&mut self, c: char) {
        let mut p = Punct::new(c, Spacing::Alone);
        p.set_span(self.span);
        self.push(TokenTree::Punct(p));
    }

    /// An attribute value: a string literal of its own.
    fn value(&mut self, text: &str) {
        let lit = with_span(Literal::string(text), self.span);
        self.push(TokenTree::Literal(lit));
    }

    fn run(&mut self) -> Result<(), Error> {
        while self.pos < self.src.len() {
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.comment()?;
            } else if rest.starts_with("<!") {
                self.doctype()?;
            } else if rest.starts_with("</") {
                self.close_tag()?;
            } else if rest.starts_with("<>") {
                self.punct('<');
                self.punct('>');
                self.pos += 2;
            } else if rest.starts_with('<')
                && rest[1..].starts_with(|c: char| c.is_ascii_alphabetic())
            {
                self.open_tag()?;
            } else if rest.starts_with("@@{") {
                self.text.push_str("@{");
                self.pos += 3;
            } else if rest.starts_with("@{") {
                let at = self.pos;
                let expr = self.rust_value(at)?;
                self.punct('@');
                self.push(expr);
            } else if rest.starts_with('{') {
                let at = self.pos;
                let source = self.client_binding(at)?;
                self.check_binding(at, None, &source)?;
                self.push(client_marker(&source, self.span));
            } else {
                self.text();
            }
        }
        // An element left open, or closed by the wrong tag, is the grammar's
        // to report: it names both tags.
        Ok(())
    }

    /// `<!-- … -->` — a note in the template, not sent to the browser.
    fn comment(&mut self) -> Result<(), Error> {
        let at = self.pos;
        match self.rest().find("-->") {
            Some(end) => {
                self.pos += end + 3;
                Ok(())
            }
            None => self.fail(at, "`<!--` is never closed — expected `-->`"),
        }
    }

    /// `<!DOCTYPE html>`
    fn doctype(&mut self) -> Result<(), Error> {
        let at = self.pos;
        let Some(end) = self.rest().find('>') else {
            return self.fail(at, "unterminated doctype — expected `>`");
        };
        let words: Vec<String> = self.rest()[2..end]
            .split_whitespace()
            .map(str::to_string)
            .collect();
        self.punct('<');
        self.punct('!');
        for word in words {
            if !is_ident(&word) {
                return self.fail(at, format!("`{word}` cannot appear in a doctype"));
            }
            self.push(TokenTree::Ident(Ident::new(&word, self.span)));
        }
        self.punct('>');
        self.pos += end + 1;
        Ok(())
    }

    /// `</name>` or `</>`.
    fn close_tag(&mut self) -> Result<(), Error> {
        let at = self.pos;
        self.pos += 2;
        self.punct('<');
        self.punct('/');
        if self.rest().starts_with('>') {
            self.pos += 1;
            self.punct('>');
            return Ok(());
        }
        let name = self.name();
        if name.is_empty() {
            return self.fail(at, "expected a tag name after `</`");
        }
        self.name_tokens(at, &name, &['-', '.'])?;
        self.skip_space();
        if !self.rest().starts_with('>') {
            return self.fail(at, format!("expected `>` to finish `</{name}`"));
        }
        self.pos += 1;
        self.punct('>');
        if let Some(index) = self.open.iter().rposition(|open| *open == name) {
            self.open.truncate(index);
        }
        Ok(())
    }

    /// `<name attr="…" …>` or `<name … />`, and a raw-text body after it.
    fn open_tag(&mut self) -> Result<(), Error> {
        let at = self.pos;
        self.pos += 1;
        self.punct('<');
        let name = self.name();
        self.name_tokens(at, &name, &['-', '.'])?;

        loop {
            self.skip_space();
            let rest = self.rest();
            if rest.is_empty() {
                return self.fail(at, format!("`<{name}` is never finished — expected `>`"));
            }
            if rest.starts_with("/>") {
                self.pos += 2;
                self.punct('/');
                self.punct('>');
                return Ok(());
            }
            if rest.starts_with('>') {
                self.pos += 1;
                self.punct('>');
                break;
            }
            self.attribute(&name)?;
        }

        let lower = name.to_ascii_lowercase();
        if RAW_TEXT.contains(&lower.as_str()) {
            self.raw_text_body(at, &lower)?;
        } else {
            self.open.push(name);
        }
        Ok(())
    }

    /// The body of a `<script>` or `<style>`, as a raw string literal, and
    /// its closing tag.
    fn raw_text_body(&mut self, at: usize, name: &str) -> Result<(), Error> {
        let close = format!("</{name}");
        let Some(end) = self.rest().to_ascii_lowercase().find(&close) else {
            return self.fail(at, format!("`<{name}>` is never closed"));
        };
        let body = &self.rest()[..end];
        if !body.trim().is_empty() {
            let hashes = (1..)
                .map(|n| "#".repeat(n))
                .find(|h| !body.contains(&format!("\"{h}")))
                .expect("a finite body leaves some hash count unused");
            let repr = format!("r{hashes}\"{body}\"{hashes}");
            let lit = Literal::from_str(&repr).map_err(|_| Error {
                span: self.span,
                msg: format!(
                    "rahti: {} — this `<{name}>` body cannot be read",
                    self.at(at)
                ),
            })?;
            self.push(TokenTree::Literal(with_span(lit, self.span)));
        }
        self.pos += end;
        self.close_tag()
    }

    /// One attribute, a spread, or a stray character.
    fn attribute(&mut self, tag: &str) -> Result<(), Error> {
        let at = self.pos;
        let rest = self.rest();

        // `@{..spread}` stands where a name would.
        if rest.starts_with("@{") {
            let expr = self.rust_value(at)?;
            self.punct('@');
            self.push(expr);
            return Ok(());
        }
        if rest.starts_with('{') {
            return self.fail(
                at,
                "a `{…}` stands where an attribute name should. Write a binding as a quoted \
                 attribute value, `name=\"{expr}\"`, and spread Rust attributes with `@{..attrs}`.",
            );
        }

        let name = self.name();
        if name.is_empty() {
            let c = self.src[at..].chars().next().unwrap_or(' ');
            return self.fail(at, format!("unexpected `{c}` in `<{tag}>`"));
        }
        if let Some(msg) = crate::authoring::attribute_mistake(tag, &name) {
            return self.fail(at, msg);
        }
        self.name_tokens(at, &name, &['-', ':'])?;

        self.skip_space();
        if !self.rest().starts_with('=') {
            return Ok(());
        }
        self.pos += 1;
        self.skip_space();
        self.punct('=');

        let value_at = self.pos;
        let rest = self.rest();
        if rest.starts_with("@{") {
            if crate::parse::is_native_event_name(&name) {
                return self.fail(value_at, crate::authoring::server_value_as_handler(&name));
            }
            let expr = self.rust_value(value_at)?;
            self.punct('@');
            self.push(expr);
            return Ok(());
        }
        if rest.starts_with('{') {
            let source = self.client_binding(value_at)?;
            return self.fail(
                value_at,
                format!(
                    "`{name}={source}` is unquoted. An attribute's binding is quoted, as in \
                     HTML: `{name}=\"{source}\"`."
                ),
            );
        }
        let quote = rest.chars().next();
        let unquoted = !matches!(quote, Some('"' | '\''));
        let value = match quote {
            Some(q @ ('"' | '\'')) => {
                let Some(end) = rest[1..].find(q) else {
                    return self.fail(value_at, format!("the value of `{name}` is never closed"));
                };
                let value = rest[1..1 + end].to_string();
                self.pos += end + 2;
                value
            }
            _ => {
                let end = rest
                    .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                    .unwrap_or(rest.len());
                if end == 0 {
                    return self.fail(
                        value_at,
                        format!(
                            "`{name}=` has no value. Write one — `{name}=\"…\"`, \
                             `{name}=\"{{expr}}\"` or `{name}=@{{expr}}` — or drop the `=` \
                             for a boolean attribute."
                        ),
                    );
                }
                let value = rest[..end].to_string();
                self.pos += end;
                value
            }
        };

        if let Some(source) = whole_binding(&value) {
            self.check_binding(value_at, Some(&name), source)?;
            self.push(client_marker(source, self.span));
            return Ok(());
        }
        let component = tag.starts_with(|c: char| c.is_ascii_uppercase());
        // A component's prop is a Rust value: unquoted, `count=3` is the
        // number and `wide=false` the bool, as written in Rust.
        if component && unquoted {
            if value == "true" || value == "false" {
                let mut group = Group::new(
                    Delimiter::Brace,
                    TokenTree::Ident(Ident::new(&value, self.span)).into(),
                );
                group.set_span(self.span);
                self.punct('@');
                self.push(TokenTree::Group(group));
                return Ok(());
            }
            if value.starts_with(|c: char| c.is_ascii_digit() || c == '-')
                && let Ok(lit) = Literal::from_str(&value)
            {
                self.push(TokenTree::Literal(with_span(lit, self.span)));
                return Ok(());
            }
        }
        if component && crate::parse::is_native_event_name(&name) {
            return self.fail(
                value_at,
                format!(
                    "`<{tag}>` is a component, and `{name}=\"{value}\"` would be a Rust prop                      named `{name}`. A component's handler is a client binding, passed to it in                      `bindings`: `{name}=\"{{{value}}}\"`."
                ),
            );
        }
        if value.contains("@{") {
            if let Some(msg) = crate::authoring::server_value_in_handler_text(&name, &value) {
                return self.fail(value_at, msg);
            }
            return self.fail(
                value_at,
                format!(
                    "`@{{…}}` inside the quoted value of `{name}` is text, never interpolated.\n  \
                     A server value is the whole value, unquoted: `{name}=@{{expr}}` — build \
                     the text in Rust (`@{{format!(…)}}`) when it mixes with literal text."
                ),
            );
        }
        // A handler is client code whatever its spelling: carried as a binding,
        // it counts toward the block's own client code — which decides whose
        // scope runs a handler written in a component's children — and ships
        // as `onclick="{save()}"`, the form the runtime binds.
        if crate::parse::is_native_event_name(&name) {
            let source = format!("{{{value}}}");
            self.check_binding(value_at, Some(&name), &source)?;
            self.push(client_marker(&source, self.span));
            return Ok(());
        }
        self.value(&value);
        Ok(())
    }

    /// The refusals a client binding gets, with its position.
    fn check_binding(&self, at: usize, attr: Option<&str>, source: &str) -> Result<(), Error> {
        let inner = &source[1..source.len() - 1];
        let code = js_outside_strings(inner);
        if code.contains("</") || code.contains("/>") {
            return self.fail(
                at,
                "a `{…}` binding renders text, never elements, and this one contains markup.\n  \
                 Show or hide an element with `hidden=\"{…}\"` on the element itself, and repeat \
                 one with `<template pp-for=\"item in items\">…</template>`. See \"PulsePoint \
                 Is Not JSX\" in docs/conventions/pulsepoint.md.",
            );
        }
        if let Some(name @ ("style" | "pp-style")) = attr {
            let trimmed = inner.trim();
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                return self.fail(
                    at,
                    format!(
                        "`{name}` takes CSS text, not an object.\n  Bind a string: \
                         `pp-style=\"{{'color: ' + color}}\"`."
                    ),
                );
            }
        }
        if inner.contains("@{") {
            return self.fail(
                at,
                "`@{…}` inside a client `{…}` binding is not interpolated: the binding is \
                 browser JavaScript, copied through as written.\n  Hand the value to the \
                 script as data — `const name = @{Json(&name)};` inside `<script>` — and \
                 write the name in the binding: `onclick=\"pick(name)\"`.",
            );
        }
        Ok(())
    }

    /// `@{expr}` at `at`, as a braced group of Rust tokens.
    fn rust_value(&mut self, at: usize) -> Result<TokenTree, Error> {
        let after = &self.src[at + 2..];
        let Some(end) = crate::parse::closing_brace_of(after) else {
            return self.fail(
                at,
                "`@{` is never closed — write `@{expr}`, or `@@{` for a literal `@{`",
            );
        };
        let source = &after[..end];
        let stream = TokenStream::from_str(source).map_err(|_| Error {
            span: self.span,
            msg: format!(
                "rahti: {} — `@{{{}}}` is not a Rust expression",
                self.at(at),
                source.trim()
            ),
        })?;
        if stream.is_empty() {
            return self.fail(at, "`@{}` is empty — write `@{expr}`");
        }
        self.pos = at + 2 + end + 1;
        let mut group = Group::new(Delimiter::Brace, stream);
        group.set_span(self.span);
        Ok(TokenTree::Group(group))
    }

    /// `{expr}` at `at`: its source, braces included, matched the way
    /// JavaScript nests — strings and template literals do not count.
    fn client_binding(&mut self, at: usize) -> Result<String, Error> {
        let Some(len) = js_braced_len(&self.src[at..]) else {
            return self.fail(
                at,
                "this `{` is never closed. A literal brace in text is written `&#123;`.",
            );
        };
        self.pos = at + len;
        Ok(self.src[at..at + len].to_string())
    }

    /// A run of text up to the next tag, binding or server value.
    fn text(&mut self) {
        let rest = self.rest();
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            let tail = &rest[i..];
            let opens_tag = c == '<'
                && (tail.starts_with("</")
                    || tail.starts_with("<!")
                    || tail.starts_with("<>")
                    || tail[1..].starts_with(|c: char| c.is_ascii_alphabetic()));
            if opens_tag || c == '{' || tail.starts_with("@{") || tail.starts_with("@@{") {
                end = i;
                break;
            }
        }
        // A `<` that opens nothing is text, written `&lt;` for the browser.
        let end = if end == 0 {
            rest.chars().next().map_or(1, char::len_utf8)
        } else {
            end
        };
        let raw = rest[..end].to_string();
        self.pos += end;

        let preserve = self
            .open
            .iter()
            .any(|name| PRESERVE_SPACE.contains(&name.to_ascii_lowercase().as_str()));
        let text = if preserve { raw } else { jsx_text(&raw) };
        self.text.push_str(&text);
    }

    /// A tag or attribute name at the cursor.
    fn name(&mut self) -> String {
        let rest = self.rest();
        let end = rest
            .find(|c: char| {
                c.is_ascii_whitespace() || matches!(c, '=' | '<' | '>' | '/' | '"' | '\'')
            })
            .unwrap_or(rest.len());
        let name = rest[..end].to_string();
        self.pos += end;
        name
    }

    /// A name as the tokens the grammar joins: identifiers between separators.
    fn name_tokens(&mut self, at: usize, name: &str, separators: &[char]) -> Result<(), Error> {
        let mut first = true;
        let mut segment = String::new();
        let mut parts: Vec<(Option<char>, String)> = Vec::new();
        let mut pending: Option<char> = None;
        for c in name.chars() {
            if separators.contains(&c) {
                parts.push((pending.take(), std::mem::take(&mut segment)));
                pending = Some(c);
            } else {
                segment.push(c);
            }
        }
        parts.push((pending, segment));
        for (separator, part) in parts {
            if let Some(c) = separator {
                self.punct(c);
            }
            if is_ident(&part) {
                self.push(TokenTree::Ident(Ident::new(&part, self.span)));
            } else if !first && part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty() {
                let lit = Literal::from_str(&part).expect("digits are an integer literal");
                self.push(TokenTree::Literal(with_span(lit, self.span)));
            } else {
                return self.fail(at, format!("`{name}` is not a name markup can carry"));
            }
            first = false;
        }
        Ok(())
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        let trimmed = rest.trim_start();
        self.pos += rest.len() - trimmed.len();
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s != "_"
}

/// `{…}` filling the whole of a quoted attribute value, if it does.
fn whole_binding(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (trimmed.starts_with('{') && js_braced_len(trimmed) == Some(trimmed.len())).then_some(trimmed)
}

/// Length of the `{…}` that opens `text`, nesting the way JavaScript does:
/// braces inside strings, template literals and comments do not count, and a
/// template literal's `${…}` does.
pub fn js_braced_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{') {
                        let inner = js_braced_len(&text[i + 1..])?;
                        i += 1 + inner;
                        continue;
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| i + 2 + end + 1);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Text as JSX treats it: whitespace that holds a line break is the
/// indentation of the markup and disappears, a space on one line is one the
/// author typed and stays, and the lines of a wrapped paragraph join with a
/// single space.
pub fn jsx_text(raw: &str) -> String {
    if !raw.contains('\n') {
        return raw.to_string();
    }
    let lines: Vec<&str> = raw.split('\n').collect();
    let last = lines.len() - 1;
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        let mut piece = *line;
        if index > 0 {
            piece = piece.trim_start();
        }
        if index < last {
            piece = piece.trim_end();
        }
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// `js` with the contents of its string and template literals blanked, so a
/// check for markup does not trip on `"</not a tag>"`.
fn js_outside_strings(js: &str) -> String {
    let mut out = String::with_capacity(js.len());
    let mut chars = js.chars();
    while let Some(c) = chars.next() {
        if matches!(c, '"' | '\'' | '`') {
            out.push(c);
            while let Some(inner) = chars.next() {
                if inner == '\\' {
                    chars.next();
                } else if inner == c {
                    break;
                }
            }
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}
