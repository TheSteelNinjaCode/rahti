//! Where an uploaded file lives, and that it does not live in memory when it
//! is large.

use super::*;

/// Spool `chunks` against a threshold, the way the multipart loop does.
async fn spool(limit: usize, spill_at: usize, chunks: &[&[u8]]) -> Result<RpcFile, SpoolError> {
    let mut spool = Spool::new(limit);
    for chunk in chunks {
        spool.push(Bytes::copy_from_slice(chunk), spill_at).await?;
    }
    Ok(RpcFile {
        name: "part.bin".to_string(),
        content_type: None,
        body: spool.finish().await?,
    })
}

fn ok(file: Result<RpcFile, SpoolError>) -> RpcFile {
    match file {
        Ok(file) => file,
        Err(SpoolError::TooLarge(limit)) => panic!("refused at {limit} bytes"),
        Err(SpoolError::Io(e)) => panic!("{e}"),
    }
}

// ------------------------------------------------------------------ spooling

/// The ordinary case: a part small enough that writing it out would cost more
/// than holding it.
#[tokio::test]
async fn a_small_part_stays_in_memory() {
    let file = ok(spool(1024, 64, &[b"hello ", b"there"]).await);

    assert!(!file.is_spilled());
    assert_eq!(file.len(), 11);
    assert_eq!(
        file.bytes().await.expect("readable"),
        Bytes::from_static(b"hello there")
    );
}

/// The point of the whole module: a part past the threshold goes to disk
/// while it arrives, so what it costs in memory is the threshold and not its
/// size.
#[tokio::test]
async fn a_part_past_the_threshold_goes_to_disk() {
    let big = vec![b'x'; 4096];
    let file = ok(spool(1 << 20, 64, &[&big]).await);

    assert!(file.is_spilled());
    assert_eq!(file.len(), 4096);
    assert_eq!(file.bytes().await.expect("readable").len(), 4096);
}

/// The chunk that crosses the threshold has to carry what was already held
/// with it, or the file starts partway through.
#[tokio::test]
async fn what_was_held_before_the_spill_is_kept() {
    let file = ok(spool(1 << 20, 8, &[b"first", b"second", b"third"]).await);

    assert!(file.is_spilled());
    assert_eq!(
        file.bytes().await.expect("readable"),
        Bytes::from_static(b"firstsecondthird")
    );
}

/// The limit is on the part, not on the chunk, so it is the running total
/// that is checked.
#[tokio::test]
async fn a_part_over_its_limit_is_refused() {
    let e = spool(10, 4, &[b"12345", b"67890", b"overflow"]).await;
    assert!(matches!(e, Err(SpoolError::TooLarge(10))));
}

/// Refused while it arrives rather than after, which is what makes the limit
/// worth having: the bytes past it are never taken.
#[tokio::test]
async fn a_part_is_refused_before_the_rest_of_it_arrives() {
    let mut spool = Spool::new(10);
    assert!(spool.push(Bytes::from_static(b"12345"), 4).await.is_ok());
    assert!(spool.push(Bytes::from_static(b"678901"), 4).await.is_err());
    assert_eq!(
        spool.len(),
        11,
        "it stopped at the chunk that crossed the limit"
    );
}

#[tokio::test]
async fn a_part_with_nothing_in_it_is_an_empty_file() {
    let file = ok(spool(1024, 64, &[]).await);
    assert!(file.is_empty());
    assert!(!file.is_spilled());
    assert!(file.bytes().await.expect("readable").is_empty());
}

// -------------------------------------------------------------------- reading

/// Both sides read back the same, which is the whole promise of the type: a
/// handler does not know or care where its file went.
#[tokio::test]
async fn both_sides_read_back_the_same() {
    let held = ok(spool(1 << 20, 1 << 20, &[b"the same bytes"]).await);
    let spilled = ok(spool(1 << 20, 4, &[b"the same bytes"]).await);

    assert!(!held.is_spilled() && spilled.is_spilled());

    for file in [&held, &spilled] {
        assert_eq!(file.len(), 14);
        assert_eq!(
            file.text().await.expect("readable").as_deref(),
            Some("the same bytes")
        );
        let read = read_all(file.reader().await.expect("a reader"))
            .await
            .expect("read");
        assert_eq!(read, b"the same bytes");
    }
}

#[tokio::test]
async fn bytes_that_are_not_text_read_as_no_text() {
    let file = ok(spool(1024, 4, &[&[0xff, 0xfe, 0xfd, 0xfc, 0xfb]]).await);
    assert!(file.is_spilled());
    assert_eq!(file.text().await.expect("readable"), None);
}

/// The stream is what an object-storage `put` takes, so it has to carry every
/// byte and it has to keep the temporary file alive while it does.
#[tokio::test]
async fn a_spilled_file_streams_without_being_read_whole() {
    use tokio_stream::StreamExt;

    let file = ok(spool(1 << 20, 16, &[&vec![b'a'; 1000], &vec![b'b'; 1000]]).await);
    assert!(file.is_spilled());

    let mut stream = Box::pin(file.into_stream().await.expect("a stream"));
    let mut total = 0;
    while let Some(chunk) = stream.next().await {
        total += chunk.expect("a chunk").len();
    }
    assert_eq!(total, 2000);
}

// --------------------------------------------------------------------- saving

fn scratch(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("rahti-test-{name}-{}", std::process::id()));
    path
}

/// A spilled file is already on disk, so saving it moves no bytes — and the
/// temporary file it was is gone afterwards.
#[tokio::test]
async fn saving_a_spilled_file_moves_it() {
    let file = ok(spool(1 << 20, 4, &[b"contents worth keeping"]).await);
    let from = match &file.body {
        FileBody::Spilled(spilled) => spilled.path().expect("a path").clone(),
        FileBody::Memory(_) => panic!("expected a spilled file"),
    };
    let to = scratch("saved");

    file.save(&to).await.expect("saved");

    assert_eq!(
        std::fs::read(&to).expect("it is there"),
        b"contents worth keeping"
    );
    assert!(!from.exists(), "the temporary file was moved, not copied");
    let _ = std::fs::remove_file(&to);
}

#[tokio::test]
async fn saving_a_held_file_writes_it() {
    let file = ok(spool(1 << 20, 1 << 20, &[b"small enough to hold"]).await);
    let to = scratch("written");

    file.save(&to).await.expect("saved");

    assert_eq!(
        std::fs::read(&to).expect("it is there"),
        b"small enough to hold"
    );
    let _ = std::fs::remove_file(&to);
}

/// A clone means something else can still read it, so there is nothing to
/// take away: it copies instead, and both files end up readable.
#[tokio::test]
async fn saving_a_shared_file_copies_it() {
    let file = ok(spool(1 << 20, 4, &[b"shared contents"]).await);
    let kept = file.clone();
    let to = scratch("copied");

    file.save(&to).await.expect("saved");

    assert_eq!(std::fs::read(&to).expect("it is there"), b"shared contents");
    assert_eq!(
        kept.bytes().await.expect("still readable"),
        Bytes::from_static(b"shared contents")
    );
    let _ = std::fs::remove_file(&to);
}

#[tokio::test]
async fn copying_leaves_the_file_where_it_was() {
    let file = ok(spool(1 << 20, 4, &[b"still mine"]).await);
    let to = scratch("copy-to");

    file.copy_to(&to).await.expect("copied");

    assert_eq!(std::fs::read(&to).expect("it is there"), b"still mine");
    assert_eq!(
        file.bytes().await.expect("still readable"),
        Bytes::from_static(b"still mine")
    );
    let _ = std::fs::remove_file(&to);
}

/// The end of the request is the end of the temporary file, whether the
/// handler saved it or not.
#[tokio::test]
async fn a_dropped_file_takes_its_temporary_file_with_it() {
    let file = ok(spool(1 << 20, 4, &[b"transient"]).await);
    let path = match &file.body {
        FileBody::Spilled(spilled) => spilled.path().expect("a path").clone(),
        FileBody::Memory(_) => panic!("expected a spilled file"),
    };
    assert!(path.exists());

    drop(file);
    assert!(!path.exists(), "the temporary file outlived the upload");
}

/// Clones share the file, so it is the last one out that deletes it.
#[tokio::test]
async fn a_clone_keeps_the_temporary_file_alive() {
    let file = ok(spool(1 << 20, 4, &[b"shared"]).await);
    let path = match &file.body {
        FileBody::Spilled(spilled) => spilled.path().expect("a path").clone(),
        FileBody::Memory(_) => panic!("expected a spilled file"),
    };

    let clone = file.clone();
    drop(file);
    assert!(path.exists(), "one holder left");

    drop(clone);
    assert!(!path.exists());
}

// --------------------------------------------------------------------- limits

/// A misconfigured limit is said out loud and then ignored, rather than
/// quietly becoming zero and refusing every upload.
#[test]
fn a_limit_that_is_not_a_number_falls_back() {
    // SAFETY: the variable is read by this test alone, and this test does not
    // touch the `OnceLock`s the resolved limits are kept in.
    unsafe {
        std::env::set_var("RAHTI_TEST_LIMIT", "not a number");
        std::env::set_var("RAHTI_TEST_LIMIT_OK", "4096");
        std::env::set_var("RAHTI_TEST_LIMIT_BLANK", "  ");
    }

    assert_eq!(size_from_env("RAHTI_TEST_LIMIT"), None);
    assert_eq!(size_from_env("RAHTI_TEST_LIMIT_OK"), Some(4096));
    assert_eq!(size_from_env("RAHTI_TEST_LIMIT_BLANK"), None);
    assert_eq!(size_from_env("RAHTI_TEST_LIMIT_UNSET"), None);
}

/// A file cannot be larger than the body carrying it, whatever the two
/// variables say.
#[test]
fn the_per_file_limit_is_never_more_than_the_body_limit() {
    assert!(max_file() <= max_upload());
    assert!(spill_threshold() > 0);
}

/// Two spilled parts of one upload cannot land on the same path.
#[tokio::test]
async fn every_spilled_part_gets_its_own_file() {
    let (first, _a) = create_spill_file().await.expect("a file");
    let (second, _b) = create_spill_file().await.expect("a file");

    assert_ne!(first, second);
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
}
