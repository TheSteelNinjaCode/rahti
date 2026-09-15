//! Request-scoped server localization over embedded Fluent catalogs.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// One build-validated Fluent file embedded in the application binary.
#[derive(Clone, Copy, Debug)]
pub struct ResourceSource {
    pub locale: &'static str,
    pub source: &'static str,
}

/// One configured locale fallback chain.
#[derive(Clone, Copy, Debug)]
pub struct FallbackSource {
    pub locale: &'static str,
    pub fallbacks: &'static [&'static str],
}

type Bundle = FluentBundle<Arc<FluentResource>>;

struct Inner {
    default: String,
    supported: HashSet<String>,
    locales: Vec<String>,
    fallbacks: HashMap<String, Vec<String>>,
    bundles: HashMap<String, Bundle>,
}

/// Immutable localization state cloned into generated router layers.
#[derive(Clone)]
pub struct I18nState(Arc<Inner>);

impl fmt::Debug for I18nState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("I18nState")
            .field("default", &self.0.default)
            .field("supported", &self.0.supported)
            .finish_non_exhaustive()
    }
}

/// A named value supplied to a Fluent message.
#[derive(Clone, Debug)]
pub enum Argument {
    String(String),
    Number(f64),
}

impl From<&str> for Argument {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for Argument {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

macro_rules! number_argument {
    ($($ty:ty),+ $(,)?) => {$ (
        impl From<$ty> for Argument {
            fn from(value: $ty) -> Self { Self::Number(value as f64) }
        }
    )+ };
}

number_argument!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

#[derive(Clone)]
struct Context {
    state: I18nState,
    locale: String,
    explicit: bool,
}

tokio::task_local! {
    static CURRENT: Context;
}

fn installed() -> &'static RwLock<Option<I18nState>> {
    static INSTALLED: OnceLock<RwLock<Option<I18nState>>> = OnceLock::new();
    INSTALLED.get_or_init(|| RwLock::new(None))
}

fn default_context() -> Option<Context> {
    installed()
        .read()
        .map(|state| state.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
        .map(|state| Context {
            locale: state.0.default.clone(),
            state,
            explicit: false,
        })
}

/// Build runtime state from the sources emitted by `rahti-build`.
#[doc(hidden)]
pub fn state(
    default: &'static str,
    locales: &'static [&'static str],
    fallbacks: &'static [FallbackSource],
    resources: &'static [ResourceSource],
) -> I18nState {
    let locale_order = locales
        .iter()
        .map(|locale| canonical(locale))
        .collect::<Vec<_>>();
    let supported = locale_order.iter().cloned().collect::<HashSet<_>>();
    let fallback_map = fallbacks
        .iter()
        .map(|source| {
            (
                canonical(source.locale),
                source
                    .fallbacks
                    .iter()
                    .map(|locale| canonical(locale))
                    .collect(),
            )
        })
        .collect();

    let mut grouped: HashMap<String, Vec<&'static str>> = HashMap::new();
    for resource in resources {
        grouped
            .entry(canonical(resource.locale))
            .or_default()
            .push(resource.source);
    }

    let mut bundles = HashMap::new();
    for locale in &supported {
        let language = locale
            .parse::<LanguageIdentifier>()
            .expect("rahti-build validated the i18n locale");
        let mut bundle = FluentBundle::new_concurrent(vec![language]);
        bundle.set_use_isolating(true);
        for source in grouped.remove(locale).unwrap_or_default() {
            let resource = FluentResource::try_new(source.to_string())
                .expect("rahti-build validated the Fluent resource");
            bundle
                .add_resource(Arc::new(resource))
                .expect("rahti-build rejected duplicate Fluent message identifiers");
        }
        bundles.insert(locale.clone(), bundle);
    }

    let state = I18nState(Arc::new(Inner {
        default: canonical(default),
        supported,
        locales: locale_order,
        fallbacks: fallback_map,
        bundles,
    }));
    match installed().write() {
        Ok(mut current) => *current = Some(state.clone()),
        Err(poisoned) => *poisoned.into_inner() = Some(state.clone()),
    }
    state
}

fn canonical(locale: &str) -> String {
    locale
        .parse::<LanguageIdentifier>()
        .map(|locale| locale.to_string())
        .unwrap_or_else(|_| locale.to_string())
}

fn first_segment(path: &str) -> Option<&str> {
    path.trim_start_matches('/')
        .split('/')
        .next()
        .filter(|segment| !segment.is_empty())
}

fn locale_route(request: &Request) -> bool {
    request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|matched| {
            let path = matched.as_str();
            path == "/{locale}"
                || path.starts_with("/{locale}/")
                || path == "/{lang}"
                || path.starts_with("/{lang}/")
        })
}

fn negotiate(state: &I18nState, request: &Request) -> Option<String> {
    if locale_route(request) {
        return first_segment(request.uri().path())
            .map(canonical)
            .filter(|locale| state.0.supported.contains(locale));
    }

    let accepted = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let mut requested = accepted
        .split(',')
        .filter_map(|part| {
            let mut fields = part.trim().split(';');
            let locale = fields.next()?.trim();
            if locale == "*" || locale.is_empty() {
                return None;
            }
            let quality = fields
                .find_map(|field| field.trim().strip_prefix("q="))
                .and_then(|quality| quality.parse::<f32>().ok())
                .unwrap_or(1.0);
            (quality > 0.0).then(|| (quality, canonical(locale)))
        })
        .collect::<Vec<_>>();
    requested.sort_by(|a, b| b.0.total_cmp(&a.0));

    for (_, requested) in requested {
        if state.0.supported.contains(&requested) {
            return Some(requested);
        }
        if let Some(language) = requested.split('-').next() {
            if let Some(found) = state
                .0
                .locales
                .iter()
                .find(|supported| supported.split('-').next() == Some(language))
            {
                return Some(found.clone());
            }
        }
    }
    Some(state.0.default.clone())
}

/// Generated localization layer for pages, APIs, and page RPCs.
#[doc(hidden)]
pub async fn middleware(
    State(state): State<I18nState>,
    mut request: Request,
    next: Next,
) -> Response {
    let explicit = locale_route(&request);
    let Some(locale) = negotiate(&state, &request) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    request
        .extensions_mut()
        .insert(CurrentLocale(locale.clone()));
    request
        .extensions_mut()
        .insert(LocaleSelection { explicit });
    let context = Context {
        state,
        locale: locale.clone(),
        explicit,
    };
    let mut response = CURRENT.scope(context, next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&locale) {
        response
            .headers_mut()
            .insert(header::CONTENT_LANGUAGE, value);
    }
    response
}

/// The canonical locale attached to the current request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentLocale(pub String);

#[derive(Clone, Copy, Debug)]
struct LocaleSelection {
    explicit: bool,
}

/// Whether the URL, rather than a varying request header, owns this locale.
pub(crate) fn request_cache_safe(request: &Request) -> bool {
    request
        .extensions()
        .get::<LocaleSelection>()
        .is_none_or(|selection| selection.explicit)
}

impl CurrentLocale {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn direction(&self) -> &'static str {
        direction_for(&self.0)
    }
}

/// Read the current request locale.
pub fn locale() -> CurrentLocale {
    CURRENT
        .try_with(|context| CurrentLocale(context.locale.clone()))
        .ok()
        .or_else(|| default_context().map(|context| CurrentLocale(context.locale)))
        .expect("rahti::i18n::locale() called before an i18n-enabled router was built")
}

/// Remove the active locale prefix before application route-policy matching.
#[doc(hidden)]
pub fn policy_path(path: &str) -> String {
    CURRENT
        .try_with(|context| {
            if !context.explicit {
                return path.to_string();
            }
            let prefix = format!("/{}", context.locale);
            if path == prefix {
                "/".to_string()
            } else if path.starts_with(&format!("{prefix}/")) {
                path[prefix.len()..].to_string()
            } else {
                path.to_string()
            }
        })
        .unwrap_or_else(|_| path.to_string())
}

/// Localize a configured application target when an i18n request is active.
#[doc(hidden)]
pub fn scoped_path(path: &str) -> String {
    CURRENT
        .try_with(|context| {
            if context.explicit {
                localized_path(path)
            } else {
                path.to_string()
            }
        })
        .unwrap_or_else(|_| path.to_string())
}

/// Translate one message with no arguments.
pub fn t(key: &str) -> String {
    t_with(key, &[])
}

/// Translate one message with named arguments.
pub fn t_with(key: &str, arguments: &[(&str, Argument)]) -> String {
    CURRENT
        .try_with(|context| translate(context, key, arguments))
        .ok()
        .or_else(|| default_context().map(|context| translate(&context, key, arguments)))
        .expect("rahti::i18n::t() called before an i18n-enabled router was built")
}

fn translate(context: &Context, key: &str, arguments: &[(&str, Argument)]) -> String {
    let mut chain = vec![context.locale.clone()];
    if let Some(configured) = context.state.0.fallbacks.get(&context.locale) {
        chain.extend(configured.iter().cloned());
    }
    if !chain.contains(&context.state.0.default) {
        chain.push(context.state.0.default.clone());
    }

    for locale in chain {
        let Some(bundle) = context.state.0.bundles.get(&locale) else {
            continue;
        };
        let Some(message) = bundle.get_message(key) else {
            continue;
        };
        let Some(pattern) = message.value() else {
            continue;
        };
        let mut args = FluentArgs::new();
        for (name, value) in arguments {
            match value {
                Argument::String(value) => args.set(*name, value.as_str()),
                Argument::Number(value) => args.set(*name, FluentValue::from(*value)),
            }
        }
        let mut errors = Vec::new();
        let rendered = bundle.format_pattern(pattern, Some(&args), &mut errors);
        if errors.is_empty() {
            return rendered.into_owned();
        }
    }
    key.to_string()
}

/// Prefix an application path with the current locale.
pub fn localized_path(path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    CURRENT
        .try_with(|context| {
            if first_segment(&path)
                .map(canonical)
                .is_some_and(|locale| context.state.0.supported.contains(&locale))
            {
                path.clone()
            } else if path == "/" {
                format!("/{}", context.locale)
            } else {
                format!("/{}{}", context.locale, path)
            }
        })
        .ok()
        .or_else(|| {
            default_context().map(|context| {
                if first_segment(&path)
                    .map(canonical)
                    .is_some_and(|locale| context.state.0.supported.contains(&locale))
                {
                    path.clone()
                } else if path == "/" {
                    format!("/{}", context.locale)
                } else {
                    format!("/{}{}", context.locale, path)
                }
            })
        })
        .expect("rahti::i18n::localized_path() called before an i18n-enabled router was built")
}

/// HTML writing direction for a locale.
pub fn direction_for(locale: &str) -> &'static str {
    match locale.split(['-', '_']).next().unwrap_or("") {
        "ar" | "ckb" | "dv" | "fa" | "he" | "ku" | "ps" | "ur" => "rtl",
        _ => "ltr",
    }
}

#[cfg(test)]
#[path = "tests/i18n.rs"]
mod tests;
