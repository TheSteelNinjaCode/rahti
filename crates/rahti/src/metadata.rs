//! Typed document metadata, inherited from layouts into pages.

use std::collections::BTreeMap;

use axum::response::{IntoResponse, Response};

use crate::html::PageBody;

/// Metadata managed by Rahti in a root document's `<head>`.
///
/// Scalar values set closer to the page replace values inherited from a
/// layout. Open Graph, Twitter, and extra named metadata merge by key, so a
/// page can replace `og:title` without repeating a layout's `og:site_name`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    title: Option<String>,
    default_title: Option<String>,
    absolute_title: bool,
    title_template: Option<String>,
    description: Option<String>,
    keywords: Option<Vec<String>>,
    canonical: Option<String>,
    robots: Option<Robots>,
    open_graph: BTreeMap<String, String>,
    twitter: BTreeMap<String, String>,
    extra: BTreeMap<String, String>,
}

impl Metadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self.absolute_title = false;
        self
    }

    /// Set the title used when no descendant supplies one.
    ///
    /// Unlike [`Self::title`], a default is not formatted by the title
    /// template declared beside it.
    pub fn default_title(mut self, title: impl Into<String>) -> Self {
        self.default_title = Some(title.into());
        self
    }

    /// Set a title that does not use an inherited title template.
    pub fn absolute_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self.absolute_title = true;
        self
    }

    /// Set the template inherited by descendant metadata.
    ///
    /// `{title}` is replaced with the final non-absolute title. A template
    /// without that placeholder is ignored rather than erasing the title.
    pub fn title_template(mut self, template: impl Into<String>) -> Self {
        self.title_template = Some(template.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = Some(keywords.into_iter().map(Into::into).collect());
        self
    }

    pub fn canonical(mut self, url: impl Into<String>) -> Self {
        self.canonical = Some(url.into());
        self
    }

    pub fn robots(mut self, robots: Robots) -> Self {
        self.robots = Some(robots);
        self
    }

    /// Add an Open Graph field without the `og:` prefix.
    pub fn open_graph(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.open_graph.insert(name.into(), content.into());
        self
    }

    /// Add a Twitter card field without the `twitter:` prefix.
    pub fn twitter(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.twitter.insert(name.into(), content.into());
        self
    }

    /// Add another `<meta name="…">` value.
    pub fn meta(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.extra.insert(name.into(), content.into());
        self
    }

    /// Merge a more specific source into this one.
    pub fn merge(&mut self, child: Metadata) {
        if child.title.is_some() {
            self.title = child.title;
            self.absolute_title = child.absolute_title;
        }
        replace(&mut self.default_title, child.default_title);
        replace(&mut self.title_template, child.title_template);
        replace(&mut self.description, child.description);
        replace(&mut self.keywords, child.keywords);
        replace(&mut self.canonical, child.canonical);
        replace(&mut self.robots, child.robots);
        self.open_graph.extend(child.open_graph);
        self.twitter.extend(child.twitter);
        self.extra.extend(child.extra);
    }

    fn rendered_title(&self) -> Option<String> {
        let Some(title) = self.title.as_ref() else {
            return self.default_title.clone();
        };
        if self.absolute_title {
            return Some(title.clone());
        }
        match self.title_template.as_deref() {
            Some(template) if template.contains("{title}") => {
                Some(template.replace("{title}", title))
            }
            _ => Some(title.clone()),
        }
    }

    fn tags(&self) -> String {
        let mut out = String::new();
        if let Some(title) = self.rendered_title() {
            out.push_str("<title data-rahti-metadata=\"\">");
            escape_text(&title, &mut out);
            out.push_str("</title>");
        }
        if let Some(value) = &self.description {
            meta_name("description", value, &mut out);
        }
        if let Some(values) = &self.keywords {
            meta_name("keywords", &values.join(", "), &mut out);
        }
        if let Some(url) = &self.canonical {
            out.push_str("<link data-rahti-metadata=\"\" rel=\"canonical\" href=\"");
            escape_attr(url, &mut out);
            out.push_str("\" />");
        }
        if let Some(robots) = self.robots {
            meta_name("robots", &robots.content(), &mut out);
        }
        for (name, value) in &self.open_graph {
            out.push_str("<meta data-rahti-metadata=\"\" property=\"og:");
            escape_attr(name, &mut out);
            out.push_str("\" content=\"");
            escape_attr(value, &mut out);
            out.push_str("\" />");
        }
        for (name, value) in &self.twitter {
            meta_name(&format!("twitter:{name}"), value, &mut out);
        }
        for (name, value) in &self.extra {
            meta_name(name, value, &mut out);
        }
        out
    }
}

fn replace<T>(target: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *target = value;
    }
}

/// The two universal robots directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Robots {
    index: bool,
    follow: bool,
}

impl Robots {
    pub const fn new() -> Self {
        Self {
            index: true,
            follow: true,
        }
    }

    pub const fn index(mut self, index: bool) -> Self {
        self.index = index;
        self
    }

    pub const fn follow(mut self, follow: bool) -> Self {
        self.follow = follow;
        self
    }

    fn content(self) -> String {
        format!(
            "{}, {}",
            if self.index { "index" } else { "noindex" },
            if self.follow { "follow" } else { "nofollow" }
        )
    }
}

impl Default for Robots {
    fn default() -> Self {
        Self::new()
    }
}

/// A dynamically rendered page together with its request-specific metadata.
pub struct Page<T> {
    content: T,
    metadata: Metadata,
}

impl<T> Page<T> {
    pub fn new(content: T, metadata: Metadata) -> Self {
        Self { content, metadata }
    }
}

impl<T: IntoResponse> IntoResponse for Page<T> {
    fn into_response(self) -> Response {
        let mut response = self.content.into_response();
        response
            .extensions_mut()
            .insert(ResolvedMetadata(self.metadata));
        response
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedMetadata(pub Metadata);

/// Add a static page's metadata below its inherited layouts.
pub async fn apply_metadata(metadata: Metadata, mut response: Response) -> Response {
    if response.extensions().get::<PageBody>().is_none() {
        return response;
    }
    let mut merged = metadata;
    if let Some(dynamic) = response.extensions_mut().remove::<ResolvedMetadata>() {
        merged.merge(dynamic.0);
    }
    response.extensions_mut().insert(ResolvedMetadata(merged));
    response
}

/// Merge two metadata sources from parent to child.
///
/// Public because the generated fallback handler has no middleware stack in
/// which to accumulate its root-layout and not-found metadata.
pub fn merge_metadata(mut parent: Metadata, child: Metadata) -> Metadata {
    parent.merge(child);
    parent
}

pub(crate) fn merge_into_response(metadata: Metadata, response: &mut Response) {
    let existing = response.extensions_mut().remove::<ResolvedMetadata>();
    let mut merged = metadata;
    if let Some(existing) = existing {
        merged.merge(existing.0);
    }
    response.extensions_mut().insert(ResolvedMetadata(merged));
}

/// Inject the resolved tags and a body template used by same-shell navigation.
pub(crate) fn inject(html: String, metadata: Metadata) -> String {
    let tags = metadata.tags();
    let mut html = if tags.is_empty() {
        html
    } else if let Some(at) = close_tag_start(&html, "</head>") {
        insert(html, at, &tags)
    } else if let Some(at) = open_tag_end(&html, "<html") {
        insert(html, at, &format!("<head>{tags}</head>"))
    } else {
        format!("<head>{tags}</head>{html}")
    };

    let template = format!("<template id=\"rahti-metadata\">{tags}</template>");
    html = if let Some(at) = close_tag_start(&html, "</body>") {
        insert(html, at, &template)
    } else {
        format!("{html}{template}")
    };
    html
}

fn meta_name(name: &str, value: &str, out: &mut String) {
    out.push_str("<meta data-rahti-metadata=\"\" name=\"");
    escape_attr(name, out);
    out.push_str("\" content=\"");
    escape_attr(value, out);
    out.push_str("\" />");
}

fn open_tag_end(html: &str, name: &str) -> Option<usize> {
    html.match_indices(name).find_map(|(at, _)| {
        let rest = &html[at + name.len()..];
        let next = rest.chars().next()?;
        if !(next.is_whitespace() || next == '>' || next == '/') {
            return None;
        }
        rest.find('>').map(|end| at + name.len() + end + 1)
    })
}

fn close_tag_start(html: &str, tag: &str) -> Option<usize> {
    html.find(tag)
}

fn insert(html: String, at: usize, value: &str) -> String {
    let mut out = String::with_capacity(html.len() + value.len());
    out.push_str(&html[..at]);
    out.push_str(value);
    out.push_str(&html[at..]);
    out
}

fn escape_text(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn escape_attr(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
#[path = "tests/metadata.rs"]
mod tests;
