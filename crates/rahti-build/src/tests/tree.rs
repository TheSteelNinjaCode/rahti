//! A throwaway directory tree, for the two phases that read the disk.
//!
//! `scanner` and `components` walk real directories and panic on anything
//! they cannot make sense of, so testing them means building small app trees
//! and letting them be walked. Nothing here is compiled into a release: the
//! module is `#[cfg(test)]`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A directory under the system temp dir, removed when the test ends.
pub struct Tree {
    root: PathBuf,
}

impl Tree {
    /// A fresh tree. `label` only makes the path readable if a test leaves
    /// one behind; uniqueness comes from the counter and the clock, so tests
    /// may run in parallel.
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "rahti-build-test-{label}-{}-{nanos:x}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("cannot create the test tree");
        Tree { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Write a file, creating the directories above it.
    pub fn file(&self, relative: &str, contents: &str) -> &Self {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("cannot create a directory");
        }
        std::fs::write(&path, contents).expect("cannot write a file");
        self
    }

    pub fn dir(&self, relative: &str) -> &Self {
        std::fs::create_dir_all(self.root.join(relative)).expect("cannot create a directory");
        self
    }

    /// A `page.rs` with the one export the router needs.
    pub fn page(&self, relative: &str) -> &Self {
        self.file(relative, "pub async fn page() -> Html { todo!() }")
    }

    pub fn layout(&self, relative: &str) -> &Self {
        self.file(
            relative,
            "pub fn layout(children: Html) -> Html { children }",
        )
    }

    pub fn error(&self, relative: &str) -> &Self {
        self.file(
            relative,
            "pub fn error(error: ErrorInfo) -> Html { todo!() }",
        )
    }

    pub fn loading(&self, relative: &str) -> &Self {
        self.file(relative, "pub fn loading() -> Html { todo!() }")
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).expect("cannot read the file back")
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        // A test that panicked has already reported; failing to clean up
        // afterwards would only bury the message.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
