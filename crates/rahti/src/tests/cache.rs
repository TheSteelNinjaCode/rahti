use super::*;
use serde::{Deserialize, Serialize};

#[tokio::test]
async fn values_expire_and_zero_ttl_removes_the_old_value() {
    let store = MemoryStore::with_capacity(4);
    store
        .put(
            "short",
            b"value".to_vec(),
            Expiration::After(Duration::ZERO),
        )
        .await
        .unwrap();
    assert_eq!(store.get("short").await.unwrap(), None);

    store
        .put(
            "expires",
            b"soon".to_vec(),
            Expiration::After(Duration::from_millis(1)),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(store.get("expires").await.unwrap(), None);

    store
        .put("short", b"old".to_vec(), Expiration::Never)
        .await
        .unwrap();
    store
        .put("short", b"new".to_vec(), Expiration::After(Duration::ZERO))
        .await
        .unwrap();
    assert_eq!(store.get("short").await.unwrap(), None);
}

#[tokio::test]
async fn capacity_evicts_the_least_recently_used_entry() {
    let store = MemoryStore::with_capacity(2);
    store.put("a", vec![1], Expiration::Never).await.unwrap();
    store.put("b", vec![2], Expiration::Never).await.unwrap();
    assert_eq!(store.get("a").await.unwrap(), Some(vec![1]));
    store.put("c", vec![3], Expiration::Never).await.unwrap();

    assert_eq!(store.get("a").await.unwrap(), Some(vec![1]));
    assert_eq!(store.get("b").await.unwrap(), None);
    assert_eq!(store.get("c").await.unwrap(), Some(vec![3]));
}

#[tokio::test]
async fn remove_and_clear_report_real_store_state() {
    let store = MemoryStore::with_capacity(4);
    store.put("one", vec![1], Expiration::Never).await.unwrap();
    store.put("two", vec![2], Expiration::Never).await.unwrap();
    assert!(store.remove("one").await.unwrap());
    assert!(!store.remove("one").await.unwrap());
    store.clear().await.unwrap();
    assert!(store.is_empty());
}

#[tokio::test]
async fn empty_and_nul_keys_are_refused() {
    let store = MemoryStore::default();
    assert!(
        store
            .get("  ")
            .await
            .unwrap_err()
            .to_string()
            .contains("empty")
    );
    assert!(
        store
            .put("bad\0key", vec![], Expiration::Never)
            .await
            .unwrap_err()
            .to_string()
            .contains("NUL")
    );
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUser {
    id: u64,
    name: String,
}

#[tokio::test]
async fn typed_facade_round_trips_json_through_the_configured_store() {
    static GLOBAL_TEST: Mutex<()> = Mutex::new(());
    let _guard = GLOBAL_TEST.lock().unwrap();
    configure(MemoryStore::with_capacity(8));

    let user = CachedUser {
        id: 42,
        name: "Ada".to_string(),
    };
    put("users:42", &user, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(get::<CachedUser>("users:42").await.unwrap(), Some(user));
    assert!(remove("users:42").await.unwrap());
    assert_eq!(get::<CachedUser>("users:42").await.unwrap(), None);
}

#[test]
#[should_panic(expected = "greater than zero")]
fn memory_capacity_cannot_be_zero() {
    MemoryStore::with_capacity(0);
}
