# Cache Stores

## Foundation

`rahti::cache` is an application-data cache with a replaceable backend. The
framework provides a bounded process-memory store by default, so caching works
without configuration:

```rust
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::rahti::cache;

#[derive(Serialize, Deserialize)]
struct ProductSummary {
    id: u64,
    name: String,
}

cache::put(
    "products:42",
    &ProductSummary { id: 42, name: "Desk".into() },
    Duration::from_secs(300),
).await?;

let product = cache::get::<ProductSummary>("products:42").await?;
cache::remove("products:42").await?;
```

The typed facade serializes values as JSON. Consequently a value written as
one Rust type must be read as a compatible type. Serialization failures and
backend failures are `CacheError`s, not cache misses. A miss or an expired key
is `Ok(None)`.

The complete facade is:

| Operation | Behavior |
| --- | --- |
| `cache::get::<T>(key)` | Read and deserialize, returning `Option<T>` |
| `cache::put(key, &value, duration)` | Store until the duration passes |
| `cache::put_forever(key, &value)` | Store without time expiry |
| `cache::remove(key)` | Remove one key and report whether it existed |
| `cache::clear()` | Clear the backend namespace owned by this application |
| `cache::store()` | Reach the installed raw byte store |

A zero duration removes an existing value and retains no replacement. Empty
keys, whitespace-only keys, and keys containing a NUL byte are rejected. Use
stable, namespaced keys such as `products:42` or `tenant:7:products:42` rather
than display text.

## Default Memory Store

`MemoryStore` is process-local, bounded, and safe to share between async
request tasks. It lazily removes expired entries and evicts the least recently
used entry when capacity is full. The default capacity is 10,000 entries.

```rust
use crate::rahti::cache::{self, MemoryStore};

cache::configure(MemoryStore::with_capacity(25_000));
```

The memory store is deliberately not persistent or distributed. Every process
has different values, and restarting loses all of them. That is correct for
derived data whose absence only costs recomputation. It is not correct for
sessions, authorization state, jobs, durable rate limits, or any value whose
loss changes application correctness.

`put_forever` means no time-based expiry, not guaranteed retention: a bounded
memory store may still evict the value. Treat every cache read as optional.

## Replacing the Backend

An adapter implements the async `CacheStore` byte contract and is installed
once during `initialize_application`, before the router starts serving:

```rust
pub async fn initialize_application() -> Result<ApplicationRuntime, StartupError> {
    // Construct and verify the application-owned Redis adapter here.
    // rahti::cache::configure(redis_store);

    Ok(ApplicationRuntime {
        router: routes::router(),
        host: routes::HOST,
        port: routes::PORT,
    })
}
```

`CacheStore` uses opaque `Vec<u8>` values and `CacheFuture`, leaving transport,
connection pooling, key prefixes, and serialization outside the framework
contract. `Expiration::After` and `Expiration::Never` are the only lifetime
shapes an adapter must translate. `clear()` means clearing only the namespace
or database that adapter was configured to own; it must not flush unrelated
applications on a shared server.

Rahti currently ships the store contract and memory implementation. It does
not ship a Redis client or create cache configuration in
`rahti.config.json`. An application that adds an external adapter owns its
dependency, credentials, connection policy, and startup failure behavior.

## Page Caching

A page opts into caching its complete rendered HTML document with one exact
export in its `page.rs`:

```rust
use crate::rahti::cache::PageCache;

pub fn cache() -> PageCache {
    PageCache::seconds(60).tags(["catalog", "featured-products"])
}

pub async fn page() -> Html {
    // Render public, deterministic content.
}
```

Use `PageCache::new(Duration)` for another duration. Every page policy has a
finite lifetime so entries made unreachable by revalidation are eventually
removed by the backend. Omitting `cache()` disables page caching. A zero
lifetime is a startup error rather than another spelling of disabled caching.

Rahti stores the response only when all of these conditions hold:

- the request is an anonymous `GET`;
- it has no `Authorization` or `Range` header and does not request cache
  bypass with `Cache-Control` or `Pragma`;
- the response is a successful Rahti HTML page, is at most 4 MiB, and sets no
  cookie;
- the response is not compressed, partial, private, or marked `no-store` or
  `no-cache`, and does not vary on a request header other than
  `Accept-Encoding`.

Authenticated sessions always bypass the page cache. Redirects, errors,
RPCs, API `route.rs` responses, streams, and uploads are never page-cached.
Development mode and `cargo rahti build --static` also render directly. This
keeps live diagnostics and static output truthful.

The key contains the request host and complete path plus query string, so
`/products?page=1` and `/products?page=2` are different entries. It does not
contain arbitrary cookies, locale headers, or application-defined request
state. A page that varies on any of those values must not export `cache()`.

`X-Rahti-Cache` reports `MISS` when Rahti rendered and stored a page, `HIT`
when it reused one, and `BYPASS` when a declared cache policy was unsafe or
unavailable for that request or response. A `HEAD` request can reuse an
existing `GET` entry without sending its body; a `HEAD` miss is not stored.

## Page Revalidation

Revalidate only after the authoritative write succeeds:

```rust
use crate::rahti::cache;

save_product(input).await?;

// Invalidates every query-string variant of this concrete URL path.
cache::revalidate_path("/products").await?;

// Invalidates every cached page that declared this tag.
cache::revalidate_tag("catalog").await?;
```

`revalidate_path` takes a concrete URL path beginning with `/`, without a
query or fragment. It does not accept a route pattern. Tags are application
defined, non-empty strings. Keep them stable and use one vocabulary for page
policies and mutations.

Revalidation advances a generation token instead of scanning the backend.
Old entries become unreachable immediately and disappear later through their
TTL or backend eviction. A shared page cache therefore also requires shared,
non-expiring generation tokens; an external adapter must not evict a current
generation token while an older page entry can still be returned.

Page-cache reads, writes, and decoding fail open: Rahti logs a `page-cache`
warning and renders the request. Revalidation is different because callers
need to know whether stale content was invalidated; its functions return a
`CacheResult<()>` that application code must handle.

## Cache Safety

The data cache is explicit, and page caching is opt-in. Rahti does not cache
rendered HTML merely because a route is a `GET`: request output may contain
session or user-specific data. If application code caches a value itself, its
key must include every identity, tenant, locale, permission, and input
dimension that can change the result.

Never make correctness or authorization depend on a cache entry. Invalidate
affected keys after the authoritative write succeeds. `clear()` is useful for
tests and operational invalidation, but broad invalidation is not a substitute
for namespaced keys and precise `remove()` calls.

Concurrent misses may compute the same value more than once. Page tags apply
only to `PageCache`; the typed data facade has no tag index. The foundation
does not promise single-flight locking, stale-while-revalidate, or a
distributed adapter. Do not infer those behaviors from `get` and `put`.
