//! Application caching behind a replaceable, asynchronous store contract.
//!
//! The default store is bounded process memory. Applications can install a
//! shared store during startup without changing callers of [`get`], [`put`],
//! [`remove`], or [`clear`]. Values use JSON at the typed boundary; store
//! adapters only move opaque bytes.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

mod page;
pub use page::{
    CACHE_HEADER, PageCache, middleware as page_middleware, revalidate_path, revalidate_tag,
};

/// Maximum entries held by the default process-memory store.
pub const DEFAULT_MEMORY_CAPACITY: usize = 10_000;

/// A future returned by a cache backend.
pub type CacheFuture<'a, T> = Pin<Box<dyn Future<Output = CacheResult<T>> + Send + 'a>>;

/// Result returned by cache operations.
pub type CacheResult<T> = std::result::Result<T, CacheError>;

/// Why a cache operation could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheError {
    message: String,
}

impl CacheError {
    /// Construct an adapter error with a message suitable for an application
    /// log. Adapters should include the failed operation, but never a secret
    /// value.
    pub fn backend(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn serialization(operation: &str, key: &str, error: impl fmt::Display) -> Self {
        Self {
            message: format!("cannot {operation} cache key `{key}` as JSON: {error}"),
        }
    }
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CacheError {}

/// How long a stored value remains available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiration {
    /// Keep the value until it is removed, cleared, or evicted by the store.
    Never,
    /// Expire the value after this duration. Zero removes an existing value
    /// and does not retain the new one.
    After(Duration),
}

impl From<Duration> for Expiration {
    fn from(value: Duration) -> Self {
        Self::After(value)
    }
}

/// Backend-neutral storage for opaque cache bytes.
///
/// Implementations must be safe to share between request tasks. A miss and an
/// expired entry both return `Ok(None)`. Store failures are errors rather than
/// misses so callers can decide whether stale or unavailable infrastructure is
/// acceptable for their operation.
pub trait CacheStore: Send + Sync + 'static {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>>;
    fn put<'a>(
        &'a self,
        key: &'a str,
        value: Vec<u8>,
        expiration: Expiration,
    ) -> CacheFuture<'a, ()>;
    fn remove<'a>(&'a self, key: &'a str) -> CacheFuture<'a, bool>;
    fn clear(&self) -> CacheFuture<'_, ()>;
}

#[derive(Debug)]
struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
    touched: u64,
}

#[derive(Debug, Default)]
struct MemoryState {
    entries: HashMap<String, Entry>,
    clock: u64,
}

impl MemoryState {
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn prune_expired(&mut self, now: Instant) {
        self.entries
            .retain(|_, entry| entry.expires_at.is_none_or(|at| at > now));
    }
}

/// A bounded, process-local least-recently-used cache.
///
/// It is immediately available with no configuration. It is not shared
/// between processes and is empty after a restart; install another
/// [`CacheStore`] when either property matters.
#[derive(Debug)]
pub struct MemoryStore {
    capacity: usize,
    state: Mutex<MemoryState>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_MEMORY_CAPACITY)
    }
}

impl MemoryStore {
    /// Construct a store holding at most `capacity` entries.
    ///
    /// # Panics
    ///
    /// Panics when capacity is zero, because such a store could never satisfy
    /// a successful `put` contract.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "a memory cache capacity must be greater than zero"
        );
        Self {
            capacity,
            state: Mutex::new(MemoryState::default()),
        }
    }

    /// Number of unexpired entries currently retained.
    pub fn len(&self) -> usize {
        let mut state = self.state();
        state.prune_expired(Instant::now());
        state.entries.len()
    }

    /// Whether no unexpired entry is retained.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        // A panic in one request must not permanently disable an optional
        // cache for every later request. The protected values remain owned.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CacheStore for MemoryStore {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            validate_key(key)?;
            let mut state = self.state();
            state.prune_expired(Instant::now());
            let touched = state.tick();
            let Some(entry) = state.entries.get_mut(key) else {
                return Ok(None);
            };
            entry.touched = touched;
            Ok(Some(entry.value.clone()))
        })
    }

    fn put<'a>(
        &'a self,
        key: &'a str,
        value: Vec<u8>,
        expiration: Expiration,
    ) -> CacheFuture<'a, ()> {
        Box::pin(async move {
            validate_key(key)?;
            let mut state = self.state();
            let now = Instant::now();
            state.prune_expired(now);

            let expires_at = match expiration {
                Expiration::Never => None,
                Expiration::After(duration) if duration.is_zero() => {
                    state.entries.remove(key);
                    return Ok(());
                }
                Expiration::After(duration) => {
                    Some(now.checked_add(duration).ok_or_else(|| {
                        CacheError::backend("cache expiration is too far in the future")
                    })?)
                }
            };

            if !state.entries.contains_key(key) && state.entries.len() == self.capacity {
                if let Some(oldest) = state
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.touched)
                    .map(|(key, _)| key.clone())
                {
                    state.entries.remove(&oldest);
                }
            }
            let touched = state.tick();
            state.entries.insert(
                key.to_string(),
                Entry {
                    value,
                    expires_at,
                    touched,
                },
            );
            Ok(())
        })
    }

    fn remove<'a>(&'a self, key: &'a str) -> CacheFuture<'a, bool> {
        Box::pin(async move {
            validate_key(key)?;
            let mut state = self.state();
            state.prune_expired(Instant::now());
            Ok(state.entries.remove(key).is_some())
        })
    }

    fn clear(&self) -> CacheFuture<'_, ()> {
        Box::pin(async move {
            self.state().entries.clear();
            Ok(())
        })
    }
}

fn validate_key(key: &str) -> CacheResult<()> {
    if key.trim().is_empty() {
        return Err(CacheError::backend("cache keys cannot be empty"));
    }
    if key.contains('\0') {
        return Err(CacheError::backend("cache keys cannot contain a NUL byte"));
    }
    Ok(())
}

fn configured_store() -> &'static RwLock<Arc<dyn CacheStore>> {
    static STORE: OnceLock<RwLock<Arc<dyn CacheStore>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(Arc::new(MemoryStore::default())))
}

/// Install the process-wide cache backend.
///
/// Call this during application initialization, before request tasks start.
/// Reconfiguration is supported so test setup can install an isolated store.
pub fn configure(store: impl CacheStore) {
    *configured_store()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(store);
}

/// The currently installed backend.
pub fn store() -> Arc<dyn CacheStore> {
    configured_store()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Read and deserialize a typed value. Expired keys are misses.
pub async fn get<T: DeserializeOwned>(key: &str) -> CacheResult<Option<T>> {
    validate_key(key)?;
    let Some(value) = store().get(key).await? else {
        return Ok(None);
    };
    serde_json::from_slice(&value)
        .map(Some)
        .map_err(|error| CacheError::serialization("read", key, error))
}

/// Serialize and store a value for the requested lifetime.
pub async fn put<T: Serialize + ?Sized>(
    key: &str,
    value: &T,
    expiration: impl Into<Expiration>,
) -> CacheResult<()> {
    validate_key(key)?;
    let value = serde_json::to_vec(value)
        .map_err(|error| CacheError::serialization("write", key, error))?;
    store().put(key, value, expiration.into()).await
}

/// Store a value without a time-based expiration.
pub async fn put_forever<T: Serialize + ?Sized>(key: &str, value: &T) -> CacheResult<()> {
    put(key, value, Expiration::Never).await
}

/// Remove one key. Returns whether the backend removed a value.
pub async fn remove(key: &str) -> CacheResult<bool> {
    validate_key(key)?;
    store().remove(key).await
}

/// Remove every value in the installed cache backend.
///
/// A shared adapter clears the namespace/database it was configured to own,
/// not necessarily an entire external server.
pub async fn clear() -> CacheResult<()> {
    store().clear().await
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;
