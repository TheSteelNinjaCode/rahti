//! Opt-in caching for complete, anonymously rendered page responses.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, HttpBody, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CacheError, CacheResult, CacheStore, Expiration, store};
use crate::html::PageBody;

/// Response header describing what the server-side page cache did.
pub const CACHE_HEADER: &str = "x-rahti-cache";

const PAGE_PREFIX: &str = "__rahti:page:v1:";
const PATH_GENERATION_PREFIX: &str = "__rahti:page-path:v1:";
const TAG_GENERATION_PREFIX: &str = "__rahti:page-tag:v1:";
const MAX_CACHED_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Cache policy returned by a page's `pub fn cache() -> PageCache` export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCache {
    expiration: Expiration,
    tags: Vec<String>,
}

impl PageCache {
    /// Cache a page for `ttl`. A zero lifetime is rejected because omitting
    /// the page's `cache()` export is the unambiguous way to turn caching off.
    pub fn new(ttl: Duration) -> Self {
        assert!(
            !ttl.is_zero(),
            "a page cache lifetime must be greater than zero"
        );
        Self {
            expiration: Expiration::After(ttl),
            tags: Vec::new(),
        }
    }

    /// Cache a page for this many seconds.
    pub fn seconds(ttl: u64) -> Self {
        Self::new(Duration::from_secs(ttl))
    }

    /// Add tags that can invalidate related pages together.
    pub fn tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags.extend(tags.into_iter().map(Into::into));
        for tag in &self.tags {
            assert!(
                valid_tag(tag),
                "page cache tags cannot be empty or contain a NUL byte"
            );
        }
        self.tags.sort();
        self.tags.dedup();
        self
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPage {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

/// Generated page-cache middleware.
///
/// Development and static export always render the application directly.
pub async fn middleware(State(policy): State<PageCache>, request: Request, next: Next) -> Response {
    if crate::dev::enabled() || crate::static_export::static_output().is_some() {
        return next.run(request).await;
    }
    serve(policy, request, next, store()).await
}

async fn serve(
    policy: PageCache,
    request: Request,
    next: Next,
    cache: Arc<dyn CacheStore>,
) -> Response {
    let method = request.method().clone();
    if !request_is_cacheable(&request) {
        return marked(next.run(request).await, "BYPASS");
    }

    // Do not borrow `Request<Body>` across an await: Body is Send but not
    // Sync, while Axum middleware futures have to be Send.
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let path = request.uri().path().to_string();
    let uri = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(&path)
        .to_string();
    let key = match page_key(cache.as_ref(), &policy, &host, &uri, &path).await {
        Ok(key) => key,
        Err(error) => {
            cache_warning("read page generations", &error);
            return marked(next.run(request).await, "BYPASS");
        }
    };

    match read_page(cache.as_ref(), &key).await {
        Ok(Some(page)) => match page.into_response(method == Method::HEAD) {
            Ok(response) => return response,
            Err(error) => {
                cache_warning("decode page", &error);
                let _ = cache.remove(&key).await;
            }
        },
        Ok(None) => {}
        Err(error) => {
            // A corrupt value cannot become a permanent outage. Remove it,
            // render normally, and let this request replace it if possible.
            cache_warning("read page", &error);
            let _ = cache.remove(&key).await;
        }
    }

    let response = next.run(request).await;
    if method == Method::HEAD {
        return marked(response, "MISS");
    }
    cache_response(cache.as_ref(), key, policy.expiration, response).await
}

fn request_is_cacheable(request: &Request) -> bool {
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        || crate::auth::is_authenticated()
        || request.headers().contains_key(header::AUTHORIZATION)
        || request.headers().contains_key(header::RANGE)
        || !crate::i18n::request_cache_safe(request)
    {
        return false;
    }

    !header_contains_token(request.headers(), header::CACHE_CONTROL, "no-cache")
        && !header_contains_token(request.headers(), header::CACHE_CONTROL, "no-store")
        && !header_contains_token(request.headers(), header::CACHE_CONTROL, "max-age=0")
        && !header_contains_token(request.headers(), header::PRAGMA, "no-cache")
}

async fn page_key(
    cache: &dyn CacheStore,
    policy: &PageCache,
    host: &str,
    uri: &str,
    path: &str,
) -> CacheResult<String> {
    let mut material = String::new();
    material.push_str(host);
    material.push('\n');
    material.push_str(uri);
    material.push('\n');
    material.push_str(&generation(cache, &generation_key(PATH_GENERATION_PREFIX, path)).await?);
    for tag in &policy.tags {
        material.push('\n');
        material.push_str(tag);
        material.push('=');
        material.push_str(&generation(cache, &generation_key(TAG_GENERATION_PREFIX, tag)).await?);
    }
    Ok(format!("{PAGE_PREFIX}{}", digest(&material)))
}

async fn generation(cache: &dyn CacheStore, key: &str) -> CacheResult<String> {
    match cache.get(key).await? {
        Some(value) => String::from_utf8(value)
            .map_err(|error| CacheError::backend(format!("invalid revalidation token: {error}"))),
        None => Ok("0".to_string()),
    }
}

async fn read_page(cache: &dyn CacheStore, key: &str) -> CacheResult<Option<StoredPage>> {
    let Some(value) = cache.get(key).await? else {
        return Ok(None);
    };
    serde_json::from_slice(&value)
        .map(Some)
        .map_err(|error| CacheError::backend(format!("invalid cached page: {error}")))
}

async fn cache_response(
    cache: &dyn CacheStore,
    key: String,
    expiration: Expiration,
    response: Response,
) -> Response {
    if !response_is_cacheable(&response) {
        return marked(response, "BYPASS");
    }
    let upper = response.body().size_hint().upper();
    if upper.is_none_or(|bytes| bytes as usize > MAX_CACHED_PAGE_BYTES) {
        return marked(response, "BYPASS");
    }

    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, MAX_CACHED_PAGE_BYTES).await {
        Ok(body) => body,
        Err(error) => {
            cache_warning("buffer page", &CacheError::backend(error.to_string()));
            return marked(StatusCode::INTERNAL_SERVER_ERROR.into_response(), "BYPASS");
        }
    };
    let stored = StoredPage::new(parts.status, &parts.headers, &body);
    let encoded = match serde_json::to_vec(&stored) {
        Ok(encoded) => encoded,
        Err(error) => {
            cache_warning("encode page", &CacheError::backend(error.to_string()));
            return marked(Response::from_parts(parts, Body::from(body)), "BYPASS");
        }
    };
    if let Err(error) = cache.put(&key, encoded, expiration).await {
        cache_warning("write page", &error);
        return marked(Response::from_parts(parts, Body::from(body)), "BYPASS");
    }
    marked(Response::from_parts(parts, Body::from(body)), "MISS")
}

fn response_is_cacheable(response: &Response) -> bool {
    response.status() == StatusCode::OK
        && response.extensions().get::<PageBody>().is_some()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/html"))
            })
        && !response.headers().contains_key(header::SET_COOKIE)
        && !response.headers().contains_key(header::CONTENT_RANGE)
        && !response.headers().contains_key(header::CONTENT_ENCODING)
        && !header_contains_token(response.headers(), header::CACHE_CONTROL, "private")
        && !header_contains_token(response.headers(), header::CACHE_CONTROL, "no-store")
        && !header_contains_token(response.headers(), header::CACHE_CONTROL, "no-cache")
        && !header_contains_token(response.headers(), header::CACHE_CONTROL, "max-age=0")
        && vary_is_safe(response.headers())
}

fn vary_is_safe(headers: &HeaderMap) -> bool {
    headers.get_all(header::VARY).iter().all(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .all(|name| name.trim().eq_ignore_ascii_case("accept-encoding"))
        })
    })
}

fn header_contains_token(headers: &HeaderMap, name: HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|part| {
                let part = part.trim();
                part.eq_ignore_ascii_case(token)
                    || (!token.contains('=')
                        && part
                            .split_once('=')
                            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(token)))
            })
        })
    })
}

impl StoredPage {
    fn new(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Self {
        let headers = headers
            .iter()
            .filter(|(name, _)| stored_header(name))
            .map(|(name, value)| (name.to_string(), B64.encode(value.as_bytes())))
            .collect();
        Self {
            status: status.as_u16(),
            headers,
            body: B64.encode(body),
        }
    }

    fn into_response(self, head: bool) -> CacheResult<Response> {
        let status = StatusCode::from_u16(self.status)
            .map_err(|error| CacheError::backend(format!("invalid cached status: {error}")))?;
        let body = B64
            .decode(self.body)
            .map_err(|error| CacheError::backend(format!("invalid cached body: {error}")))?;
        let body_len = body.len();
        let mut response = Response::builder().status(status);
        if let Some(headers) = response.headers_mut() {
            for (name, value) in self.headers {
                let name = HeaderName::try_from(name).map_err(|error| {
                    CacheError::backend(format!("invalid cached header name: {error}"))
                })?;
                let value = B64.decode(value).map_err(|error| {
                    CacheError::backend(format!("invalid cached header value: {error}"))
                })?;
                let value = HeaderValue::from_bytes(&value).map_err(|error| {
                    CacheError::backend(format!("invalid cached header value: {error}"))
                })?;
                headers.append(name, value);
            }
            if head {
                if let Ok(value) = HeaderValue::from_str(&body_len.to_string()) {
                    headers.insert(header::CONTENT_LENGTH, value);
                }
            }
            headers.insert(CACHE_HEADER, HeaderValue::from_static("HIT"));
        }
        let mut response = response
            .body(if head {
                Body::empty()
            } else {
                Body::from(body)
            })
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        response.extensions_mut().insert(PageBody);
        Ok(response)
    }
}

fn stored_header(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "date"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | CACHE_HEADER
    )
}

fn marked(mut response: Response, value: &'static str) -> Response {
    response
        .headers_mut()
        .insert(CACHE_HEADER, HeaderValue::from_static(value));
    response
}

fn cache_warning(operation: &str, error: &CacheError) {
    crate::dev::record_server(
        "warn",
        "page-cache",
        &format!("rahti: could not {operation}; serving the page without cache: {error}"),
    );
}

/// Invalidate every cached query variant of one concrete URL path.
pub async fn revalidate_path(path: &str) -> CacheResult<()> {
    revalidate(store().as_ref(), PATH_GENERATION_PREFIX, path, valid_path).await
}

/// Invalidate every cached page carrying `tag`.
pub async fn revalidate_tag(tag: &str) -> CacheResult<()> {
    revalidate(store().as_ref(), TAG_GENERATION_PREFIX, tag, valid_tag).await
}

async fn revalidate(
    cache: &dyn CacheStore,
    prefix: &str,
    value: &str,
    valid: fn(&str) -> bool,
) -> CacheResult<()> {
    if !valid(value) {
        return Err(CacheError::backend(if prefix == PATH_GENERATION_PREFIX {
            "revalidation paths must start with `/` and contain no query, fragment, or NUL byte"
        } else {
            "revalidation tags cannot be empty or contain a NUL byte"
        }));
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|error| {
        CacheError::backend(format!("cannot create revalidation token: {error}"))
    })?;
    cache
        .put(
            &generation_key(prefix, value),
            B64.encode(nonce).into_bytes(),
            Expiration::Never,
        )
        .await
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains(['?', '#', '\0'])
}

fn valid_tag(tag: &str) -> bool {
    !tag.trim().is_empty() && !tag.contains('\0')
}

fn generation_key(prefix: &str, value: &str) -> String {
    format!("{prefix}{}", digest(value))
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "../tests/page_cache.rs"]
mod tests;
