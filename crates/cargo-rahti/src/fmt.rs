//! Source-preserving formatting of authored `html!` blocks.
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::{
    collections::BTreeSet,
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

#[cfg(test)]
#[path = "tests/fmt.rs"]
mod tests;

pub fn run(args: &[&str]) -> Result<(), String> {
    let mut check = false;
    let mut paths = Vec::new();
    for arg in args {
        match *arg {
            "--check" => check = true,
            "--help" | "-h" => {
                println!(
                    "cargo rahti fmt [--check] [path...]\nFormats html! markup; defaults to src/. Run cargo fmt for ordinary Rust."
                );
                return Ok(());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown fmt option: {flag}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("src"));
        if let Ok(config) = fs::read_to_string("rahti.config.json") {
            let config: serde_json::Value =
                serde_json::from_str(&config).map_err(|e| format!("rahti.config.json: {e}"))?;
            if let Some(dir) = config.pointer("/app/dir").and_then(|v| v.as_str()) {
                let path = PathBuf::from(dir);
                if !path.starts_with("src") {
                    paths.push(path);
                }
            }
        }
    }
    let mut files = BTreeSet::new();
    for path in paths {
        collect(&path, &mut files)?;
    }
    // Prepare every change before writing: a malformed file must not produce
    // a partially formatted project.
    let mut changes = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let formatted = format_source(&source).map_err(|e| format!("{}: {e}", path.display()))?;
        if formatted != source {
            changes.push((path, formatted));
        }
    }
    for (path, source) in &changes {
        if check {
            println!("Would format {}", path.display());
        } else {
            fs::write(path, source).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("Formatted {}", path.display());
        }
    }
    if check && !changes.is_empty() {
        Err("html! formatting differs; run cargo rahti fmt".into())
    } else {
        Ok(())
    }
}

fn generated(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .map(|p| p.as_os_str().to_string_lossy())
        .collect();
    path.file_name().is_some_and(|n| n == "routes.rs")
        || (path.file_name().is_some_and(|n| n == "mod.rs")
            && parts
                .iter()
                .rev()
                .nth(1)
                .is_some_and(|p| matches!(p.as_ref(), "components" | "models" | "migrations")))
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        if path.file_name().is_some_and(|n| {
            ["target", ".git", ".rahti", "node_modules", "dist", "native"]
                .iter()
                .any(|s| n == *s)
        }) {
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            collect(&entry.map_err(|e| e.to_string())?.path(), files)?;
        }
    } else if path.extension().is_some_and(|e| e == "rs") && !generated(path) {
        files.insert(fs::canonicalize(path).map_err(|e| e.to_string())?);
    }
    Ok(())
}

fn punct(token: Option<&TokenTree>, c: char) -> bool {
    matches!(token, Some(TokenTree::Punct(p)) if p.as_char() == c)
}

pub(super) fn format_source(source: &str) -> Result<String, String> {
    let stream: TokenStream = source
        .parse()
        .map_err(|e| format!("cannot lex Rust: {e}"))?;
    let mut edits = Vec::new();
    find_blocks(stream, source, &mut edits)?;
    let mut out = source.to_owned();
    edits.sort_by_key(|(range, _)| range.start);
    for (range, replacement) in edits.into_iter().rev() {
        out.replace_range(range, &replacement);
    }
    Ok(out)
}

fn find_blocks(
    stream: TokenStream,
    source: &str,
    edits: &mut Vec<(Range<usize>, String)>,
) -> Result<(), String> {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (i, token) in tokens.iter().enumerate() {
        if let TokenTree::Group(group) = token {
            if i >= 2
                && matches!(&tokens[i-2], TokenTree::Ident(id) if id == "html")
                && punct(tokens.get(i - 1), '!')
            {
                let range = group.span().byte_range();
                let line = source[..range.start].rsplit('\n').next().unwrap_or("");
                let indent: String = line
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                let mut parser = Markup {
                    tokens: group.stream().into_iter().collect(),
                    pos: 0,
                    source,
                    cursor: range.start + 1,
                };
                match parser.nodes(None) {
                    Ok(nodes) => {
                        // Preserve any trailing comments before the macro delimiter.
                        let mut nodes = nodes;
                        parser.gap(range.end - 1, &mut nodes);
                        let newline = if source.contains("\r\n") {
                            "\r\n"
                        } else {
                            "\n"
                        };
                        let mut body = String::new();
                        for node in nodes {
                            node.write(&mut body, &(indent.clone() + "    "), newline);
                        }
                        let (open, close) = match group.delimiter() {
                            Delimiter::Brace => ('{', '}'),
                            Delimiter::Parenthesis => ('(', ')'),
                            Delimiter::Bracket => ('[', ']'),
                            Delimiter::None => continue,
                        };
                        edits.push((range, format!("{open}{newline}{body}{indent}{close}")));
                    }
                    // Bare text has source-sensitive whitespace. Leave the whole
                    // block untouched rather than guessing its rendered spaces.
                    Err(ParseError::BareText) => {}
                    Err(ParseError::Invalid(message)) => return Err(message),
                }
            } else {
                find_blocks(group.stream(), source, edits)?;
            }
        }
    }
    Ok(())
}

enum Node {
    Leaf(String),
    Element {
        open: String,
        attrs: Vec<String>,
        close: String,
        children: Vec<Node>,
        opaque: bool,
    },
}

impl Node {
    fn write(&self, out: &mut String, indent: &str, nl: &str) {
        match self {
            Self::Leaf(text) => {
                out.push_str(indent);
                out.push_str(text);
                out.push_str(nl);
            }
            Self::Element {
                open,
                attrs,
                close,
                children,
                opaque,
            } => {
                let opening = if attrs.is_empty() {
                    open.clone()
                } else {
                    format!("{} {}", open, attrs.join(" "))
                };
                let ending = if close.is_empty() { " />" } else { ">" };
                let leaves: Option<Vec<&str>> = children
                    .iter()
                    .map(|n| match n {
                        Self::Leaf(s)
                            if !s.contains('\n')
                                && !s.starts_with("//")
                                && !s.starts_with("/*") =>
                        {
                            Some(s.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                let inline = leaves.map(|v| v.join(""));
                out.push_str(indent);
                if !opaque && !attrs.is_empty() && indent.len() + opening.len() + ending.len() > 100
                {
                    out.push_str(open);
                    out.push_str(nl);
                    for attr in attrs {
                        out.push_str(indent);
                        out.push_str("    ");
                        out.push_str(attr);
                        out.push_str(nl);
                    }
                    out.push_str(indent);
                    out.push_str(ending.trim_start());
                } else {
                    out.push_str(&opening);
                    out.push_str(ending);
                }
                if close.is_empty() {
                    out.push_str(nl);
                    return;
                }
                if *opaque {
                    for child in children {
                        if let Self::Leaf(s) = child {
                            out.push_str(s);
                        }
                    }
                    out.push_str(close);
                    out.push_str(nl);
                } else if let Some(text) = inline.filter(|s| {
                    indent.len() + opening.len() + ending.len() + s.len() + close.len() <= 100
                }) {
                    out.push_str(&text);
                    out.push_str(close);
                    out.push_str(nl);
                } else {
                    out.push_str(nl);
                    for child in children {
                        child.write(out, &(indent.to_owned() + "    "), nl);
                    }
                    out.push_str(indent);
                    out.push_str(close);
                    out.push_str(nl);
                }
            }
        }
    }
}

enum ParseError {
    BareText,
    Invalid(String),
}
type Parsed<T> = Result<T, ParseError>;
struct Markup<'a> {
    tokens: Vec<TokenTree>,
    pos: usize,
    source: &'a str,
    cursor: usize,
}
impl Markup<'_> {
    fn at(&self, c: char) -> bool {
        punct(self.tokens.get(self.pos), c)
    }
    fn range(&self, index: usize) -> Range<usize> {
        self.tokens[index].span().byte_range()
    }
    fn gap(&mut self, end: usize, nodes: &mut Vec<Node>) {
        let gap = self.source[self.cursor..end].trim();
        if !gap.is_empty() {
            nodes.push(Node::Leaf(gap.to_owned()));
        }
        self.cursor = end;
    }
    fn invalid<T>(&self) -> Parsed<T> {
        Err(ParseError::Invalid(
            "cannot format malformed html! markup".into(),
        ))
    }
    fn nodes(&mut self, closing: Option<&str>) -> Parsed<Vec<Node>> {
        let mut nodes = Vec::new();
        while self.pos < self.tokens.len() {
            let start = self.range(self.pos).start;
            self.gap(start, &mut nodes);
            if self.at('<') && punct(self.tokens.get(self.pos + 1), '/') {
                if closing.is_none() {
                    return self.invalid();
                }
                return Ok(nodes);
            }
            if self.at('<') {
                nodes.push(self.element()?);
            } else {
                let first = self.pos;
                if self.at('@') {
                    self.pos += 1;
                }
                match self.tokens.get(self.pos) {
                    Some(TokenTree::Literal(literal))
                        if literal.to_string().starts_with('"')
                            || literal.to_string().starts_with("r\"")
                            || literal.to_string().starts_with("r#") =>
                    {
                        self.pos += 1;
                        if matches!(self.tokens.get(self.pos), Some(TokenTree::Literal(_))) {
                            // Several adjacent literals are parsed as a bare
                            // text run, whose spaces beside tags are significant.
                            return Err(ParseError::BareText);
                        }
                    }
                    Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => self.pos += 1,
                    _ => return Err(ParseError::BareText),
                }
                let end = self.range(self.pos - 1).end;
                nodes.push(Node::Leaf(
                    self.source[self.range(first).start..end].to_owned(),
                ));
                self.cursor = end;
            }
        }
        if closing.is_some() {
            self.invalid()
        } else {
            Ok(nodes)
        }
    }
    fn name(&mut self) -> Parsed<String> {
        let first = self.pos;
        if !matches!(self.tokens.get(self.pos), Some(TokenTree::Ident(_))) {
            return self.invalid();
        }
        self.pos += 1;
        while self.at('-') || self.at(':') || self.at('.') {
            self.pos += 1;
            if !matches!(
                self.tokens.get(self.pos),
                Some(TokenTree::Ident(_)) | Some(TokenTree::Literal(_))
            ) {
                return self.invalid();
            }
            self.pos += 1;
        }
        Ok(self.source[self.range(first).start..self.range(self.pos - 1).end].to_owned())
    }
    fn element(&mut self) -> Parsed<Node> {
        let first = self.pos;
        self.pos += 1;
        if self.at('!') {
            // Preserve doctypes exactly.
            while self.pos < self.tokens.len() && !self.at('>') {
                self.pos += 1;
            }
            if self.pos == self.tokens.len() {
                return self.invalid();
            }
            self.pos += 1;
            self.cursor = self.range(self.pos - 1).end;
            return Ok(Node::Leaf(
                self.source[self.range(first).start..self.cursor].to_owned(),
            ));
        }
        let name = if self.at('>') {
            String::new()
        } else {
            self.name()?
        };
        let open = format!("<{name}");
        let mut attrs = Vec::new();
        while self.pos < self.tokens.len() && !self.at('>') && !self.at('/') {
            let start = self.pos;
            if self.at('@') {
                self.pos += 1;
                if !matches!(self.tokens.get(self.pos), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace)
                {
                    return self.invalid();
                }
                self.pos += 1;
            } else {
                self.name()?;
                if self.at('=') {
                    self.pos += 1;
                    if self.at('@') || self.at('-') {
                        self.pos += 1;
                    }
                    match self.tokens.get(self.pos) {
                        Some(TokenTree::Literal(_))
                        | Some(TokenTree::Group(_))
                        | Some(TokenTree::Ident(_)) => self.pos += 1,
                        _ => return self.invalid(),
                    }
                }
            }
            attrs.push(
                self.source[self.range(start).start..self.range(self.pos - 1).end].to_owned(),
            );
        }
        let self_closing = self.at('/');
        if self_closing {
            self.pos += 1;
        }
        if !self.at('>') {
            return self.invalid();
        }
        self.pos += 1;
        let opening_end = self.range(self.pos - 1).end;
        // Comments in an opening tag are retained by keeping its entire source.
        let opening_source = &self.source[self.range(first).start..opening_end];
        let tag_comment = (first..self.pos - 1).any(|i| {
            !self.source[self.range(i).end..self.range(i + 1).start]
                .trim()
                .is_empty()
        });
        self.cursor = opening_end;
        if self_closing {
            if tag_comment {
                return Ok(Node::Leaf(opening_source.to_owned()));
            }
            return Ok(Node::Element {
                open,
                attrs,
                close: String::new(),
                children: vec![],
                opaque: false,
            });
        }
        let raw = name == "script" || name == "style";
        let children = self.nodes(Some(&name))?;
        let close_start = self.range(self.pos).start;
        self.pos += 2;
        let actual = if self.at('>') {
            String::new()
        } else {
            self.name()?
        };
        if actual != name || !self.at('>') {
            return self.invalid();
        }
        self.pos += 1;
        self.cursor = self.range(self.pos - 1).end;
        if tag_comment {
            return Ok(Node::Leaf(
                self.source[self.range(first).start..self.cursor].to_owned(),
            ));
        }
        Ok(Node::Element {
            open,
            attrs,
            close: self.source[close_start..self.cursor].to_owned(),
            children: if raw {
                vec![Node::Leaf(self.source[opening_end..close_start].to_owned())]
            } else {
                children
            },
            opaque: raw,
        })
    }
}
