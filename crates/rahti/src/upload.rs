//! Where an uploaded file lives between arriving and being used.
//!
//! A part read with `field.bytes()` is a part held whole in memory, which
//! makes the largest upload the app allows also the memory one request can
//! cost — twice over, since a `FileList` holds every file at once. That is
//! why the limit could not be raised.
//!
//! So a part is not read whole. It is spooled: chunks accumulate in memory
//! until they pass [`spill_threshold`], and a part that passes it opens a
//! temporary file and keeps going on disk. Small parts — which is most of
//! them — never touch the filesystem and cost exactly what they did before;
//! a large one costs the threshold, whatever its size. Memory stops being a
//! function of the upload.
//!
//! What the handler gets back is the same [`RpcFile`] either way. Where the
//! bytes actually are is the difference between [`RpcFile::save`] renaming a
//! file and writing one, and it is not otherwise visible.
//!
//! ## Reading one
//!
//! [`RpcFile::bytes`] is still there and still gives the whole file, but it
//! is now `async` and fallible, because on the spilled side it is a read.
//! That it reads is the point: the type no longer hides the cost of asking
//! for every byte at once. [`RpcFile::reader`] and [`RpcFile::into_stream`]
//! are the ones that do not pay it, and [`RpcFile::save`] is better than
//! both — a spilled file that is being written to disk anyway is a rename,
//! which moves no bytes at all.
//!
//! ## Cleaning up
//!
//! A spilled file is deleted when the last [`RpcFile`] holding it is
//! dropped, which is the end of the request whether it succeeded or not.
//! [`RpcFile::save`] takes the file away from that by renaming it, so the
//! saved file is not deleted behind the handler's back.

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Bytes;
use axum::http::StatusCode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio_stream::Stream;
use tokio_util::io::ReaderStream;

use super::error::Error;

/// How large an upload may be, all its parts together, when nothing says
/// otherwise.
///
/// Generous, because an upload's cost is not its size in memory: the body is
/// written to disk as it arrives. It is still a limit rather than none, since
/// disk is also finite.
pub const DEFAULT_MAX_UPLOAD: usize = 128 * 1024 * 1024;

/// How large any one file in an upload may be, when nothing says otherwise.
///
/// Zero means "whatever [`max_upload`] is". A cap per file matters because
/// the body limit is shared: without this, one part of a `FileList` can
/// spend the whole allowance and the rest arrive empty.
pub const DEFAULT_MAX_FILE: usize = 0;

/// Where a part stops being held and starts being written.
///
/// 256 KiB is chosen so the parts that are not really files — the string
/// values `FormData` writes beside them, small images, a JSON blob — stay
/// in memory and cost no syscalls, while anything big enough for its size
/// to matter goes to disk.
pub const DEFAULT_SPILL_THRESHOLD: usize = 256 * 1024;

/// Read instead of [`DEFAULT_MAX_UPLOAD`], in bytes.
pub const MAX_UPLOAD_ENV: &str = "RAHTI_MAX_UPLOAD";
/// Read instead of [`DEFAULT_MAX_FILE`], in bytes.
pub const MAX_FILE_ENV: &str = "RAHTI_MAX_FILE";
/// Read instead of [`DEFAULT_SPILL_THRESHOLD`], in bytes.
pub const SPILL_THRESHOLD_ENV: &str = "RAHTI_SPILL_THRESHOLD";
/// Where spilled files are written. The system temporary directory when unset.
///
/// Worth setting when the temporary directory is a small tmpfs — which is
/// memory, and would undo the point of spilling — or when uploads should
/// land on the same filesystem they will be saved to, so that saving is a
/// rename rather than a copy.
pub const SPILL_DIR_ENV: &str = "RAHTI_SPILL_DIR";

/// The whole-body limit, which the generated router hangs on the rpc routes
/// as an `axum::extract::DefaultBodyLimit`.
///
/// Read from the environment once and remembered: it is asked for on the way
/// into every upload, and it cannot change while the process runs.
pub fn max_upload() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| size_from_env(MAX_UPLOAD_ENV).unwrap_or(DEFAULT_MAX_UPLOAD))
}

/// The per-file limit, resolved: [`max_upload`] when it is unset or zero,
/// and never more than that, since a file cannot outgrow the body carrying
/// it.
pub fn max_file() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(
        || match size_from_env(MAX_FILE_ENV).unwrap_or(DEFAULT_MAX_FILE) {
            0 => max_upload(),
            limit => limit.min(max_upload()),
        },
    )
}

/// Where a part stops being held in memory.
pub fn spill_threshold() -> usize {
    static VALUE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| size_from_env(SPILL_THRESHOLD_ENV).unwrap_or(DEFAULT_SPILL_THRESHOLD))
}

/// The directory spilled files are written to.
pub fn spill_dir() -> PathBuf {
    static VALUE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    VALUE
        .get_or_init(|| match std::env::var(SPILL_DIR_ENV) {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => std::env::temp_dir(),
        })
        .clone()
}

/// A size in bytes, or nothing when the variable is unset or unreadable.
///
/// A value that is not a number is a misconfiguration rather than a request
/// for the default, so it is said out loud instead of quietly ignored.
fn size_from_env(name: &str) -> Option<usize> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<usize>() {
        Ok(value) => Some(value),
        Err(_) => {
            super::dev::record_server(
                "warn",
                "upload-config",
                &format!("rahti: {name} is not a number of bytes — `{raw}`. Using the default."),
            );
            None
        }
    }
}

/// What the multipart loop is allowed to do with one part.
///
/// Resolved once per call rather than read out of the environment at every
/// part — and passed rather than reached for, so a test can spool against a
/// threshold of a few bytes without the process-wide value being involved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// The most any one file may weigh.
    pub file: usize,
    /// Where a file stops being held and starts being written.
    pub spill_at: usize,
    /// The most a part with no filename may weigh. It becomes a string, so
    /// this is small and it never spills.
    pub value: usize,
}

impl Limits {
    pub(crate) fn from_env(value: usize) -> Self {
        Limits {
            file: max_file(),
            spill_at: spill_threshold(),
            value,
        }
    }

    /// What one part gets, which depends on whether it is a file.
    pub(crate) fn for_part(&self, is_file: bool) -> (usize, usize) {
        match is_file {
            true => (self.file, self.spill_at),
            false => (self.value, usize::MAX),
        }
    }
}

/// A file the browser sent.
///
/// Take one as a parameter and `#[rpc]` hands it over: `avatar: RpcFile` for
/// a single `File`, `Vec<RpcFile>` for a `FileList`, `Option<RpcFile>` for a
/// field the caller may leave empty.
///
/// Cloning one is cheap and shares the bytes rather than copying them, which
/// is also why a clone kept past the end of the request keeps a spilled
/// file alive until it too is dropped.
#[derive(Debug, Clone)]
pub struct RpcFile {
    pub(crate) name: String,
    pub(crate) content_type: Option<String>,
    pub(crate) body: FileBody,
}

/// Where the bytes of one part ended up.
#[derive(Debug, Clone)]
pub(crate) enum FileBody {
    /// Small enough that spilling it would have cost more than holding it.
    Memory(Bytes),
    /// Past the threshold, so it lives in a temporary file. Shared, because
    /// an `RpcFile` clones — and counted, because the last one out deletes
    /// it.
    Spilled(Arc<Spilled>),
}

impl FileBody {
    fn len(&self) -> usize {
        match self {
            FileBody::Memory(bytes) => bytes.len(),
            FileBody::Spilled(file) => file.len,
        }
    }
}

/// A part that went to disk.
///
/// The path is an `Option` because [`RpcFile::save`] takes it: a renamed
/// file is the handler's now, and deleting it on drop would be deleting what
/// was just saved.
#[derive(Debug)]
pub(crate) struct Spilled {
    path: Option<PathBuf>,
    len: usize,
}

impl Drop for Spilled {
    /// The request is over, so the temporary file goes with it.
    ///
    /// Synchronous inside a drop, which is a blocking call on a runtime
    /// thread — but it is one `unlink`, it happens once per spilled part,
    /// and the alternative is spawning a task to delete a file that is
    /// already unreferenced.
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl RpcFile {
    /// One held in memory, which is what a test or a hand-built call wants.
    pub fn from_bytes(
        name: impl Into<String>,
        content_type: Option<String>,
        bytes: impl Into<Bytes>,
    ) -> Self {
        RpcFile {
            name: name.into(),
            content_type,
            body: FileBody::Memory(bytes.into()),
        }
    }

    /// The name the browser reported — chosen by whoever uploaded it, so it
    /// is input like any other. [`RpcFile::safe_name`] is the one to build a
    /// path out of.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The reported name reduced to something that cannot escape a directory:
    /// no path, no `..`, and nothing outside letters, digits, `.`, `-` and
    /// `_`. `None` when nothing usable is left.
    ///
    /// Joining an unfiltered upload name onto a directory is how a file ends
    /// up written somewhere else entirely, so this is separate from
    /// [`RpcFile::name`] and the one that reads as safe at the call site.
    pub fn safe_name(&self) -> Option<String> {
        safe_name(&self.name)
    }

    /// What the browser said this is. Reported by the client, not sniffed
    /// here, so treat it as a hint rather than as a fact.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// How many bytes arrived.
    ///
    /// Counted while the part was read, so this is the one question that is
    /// answered the same way and just as cheaply on both sides.
    pub fn len(&self) -> usize {
        self.body.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the bytes are still in memory.
    ///
    /// Not something a handler should branch on — [`RpcFile::save`] and
    /// [`RpcFile::reader`] already do the right thing on both sides — but
    /// worth being able to assert in a test.
    pub fn is_spilled(&self) -> bool {
        matches!(self.body, FileBody::Spilled(_))
    }

    /// Every byte, at once.
    ///
    /// Asynchronous and fallible because a spilled file has to be read back
    /// to answer this, and a 100 MiB upload answers it with 100 MiB of
    /// memory. Reach for [`RpcFile::save`], [`RpcFile::reader`] or
    /// [`RpcFile::into_stream`] first; this is for the cases that genuinely
    /// need the whole thing in hand, like parsing.
    pub async fn bytes(&self) -> io::Result<Bytes> {
        match &self.body {
            FileBody::Memory(bytes) => Ok(bytes.clone()),
            FileBody::Spilled(file) => Ok(Bytes::from(tokio::fs::read(file.path()?).await?)),
        }
    }

    /// The same, giving up ownership — which saves a copy when the file was
    /// small enough to have stayed in memory.
    pub async fn into_bytes(self) -> io::Result<Bytes> {
        match self.body {
            FileBody::Memory(bytes) => Ok(bytes),
            FileBody::Spilled(_) => self.bytes().await,
        }
    }

    /// The contents as text, if they are UTF-8.
    ///
    /// `Err` is "could not be read", `Ok(None)` is "read, and not text".
    pub async fn text(&self) -> io::Result<Option<String>> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8(bytes.into()).ok())
    }

    /// Read the file a piece at a time.
    ///
    /// The memory an upload costs to handle is whatever the caller reads
    /// into, rather than the size of the upload — which is the point of
    /// this over [`RpcFile::bytes`].
    pub async fn reader(&self) -> io::Result<RpcFileReader> {
        Ok(RpcFileReader(match &self.body {
            FileBody::Memory(bytes) => Source::Memory(io::Cursor::new(bytes.clone())),
            FileBody::Spilled(file) => {
                let handle = tokio::fs::File::open(file.path()?).await?;
                // The `Arc` rides along so the temporary file is not deleted
                // out from under the reader — which on Windows would not even
                // be allowed, and on Unix would leave a handle onto a file
                // with no name.
                Source::File(handle, Arc::clone(file))
            }
        }))
    }

    /// The file as a stream of chunks, which is the shape an object-storage
    /// client's `put` wants.
    ///
    /// ```ignore
    /// let body = reqwest::Body::wrap_stream(upload.into_stream().await?);
    /// ```
    pub async fn into_stream(self) -> io::Result<impl Stream<Item = io::Result<Bytes>> + Send> {
        Ok(ReaderStream::new(self.reader().await?))
    }

    /// Write the file out, and give up ownership of it.
    ///
    /// A spilled file is *renamed* into place when it can be, which moves no
    /// bytes at all and is why this consumes `self`: after the rename there
    /// is no temporary file left to read. When a rename is not possible —
    /// the destination is on another filesystem, or a clone of this file is
    /// still alive somewhere — it falls back to a streaming copy, which
    /// still never holds more than a buffer.
    ///
    /// [`RpcFile::copy_to`] is the same thing for a handler that wants to
    /// keep reading afterwards.
    pub async fn save(self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();

        let shared = match self.body {
            FileBody::Memory(bytes) => return tokio::fs::write(path, &bytes).await,
            FileBody::Spilled(shared) => shared,
        };

        // Unique means nothing else can read it afterwards, which is what
        // makes taking the file away from its own cleanup safe.
        let mut owned = match Arc::try_unwrap(shared) {
            Ok(owned) => owned,
            Err(shared) => return copy_file(&FileBody::Spilled(shared), path).await,
        };

        let from = owned.path()?.to_path_buf();
        match tokio::fs::rename(&from, path).await {
            Ok(()) => {
                // Renamed, so there is nothing at the old path to clean up.
                owned.path = None;
                Ok(())
            }
            // Across filesystems there is no rename, only a copy. Nothing is
            // taken in that case: the temporary file is still ours to delete.
            Err(_) => copy_file(&FileBody::Spilled(Arc::new(owned)), path).await,
        }
    }

    /// Write the file out and keep it.
    ///
    /// Always a copy, so [`RpcFile::save`] is the better one when the file
    /// is not needed afterwards.
    pub async fn copy_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        copy_file(&self.body, path.as_ref()).await
    }
}

impl Spilled {
    /// The path, or the error for a file that has already been saved away.
    fn path(&self) -> io::Result<&PathBuf> {
        self.path.as_ref().ok_or_else(|| {
            io::Error::other("rahti: this upload was already saved, so its bytes have moved")
        })
    }
}

/// A reported name reduced to something that cannot escape a directory.
///
/// Shared by [`RpcFile`] and [`RpcUpload`], because the name comes from the
/// same place on both wires: whoever uploaded the file.
fn safe_name(reported: &str) -> Option<String> {
    let base = reported.rsplit(['/', '\\']).next().unwrap_or(reported);

    let cleaned: String = base
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c,
            '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect();

    // A name that is only dots is `.` or `..` — a directory, not a file.
    let cleaned = cleaned.trim_matches('.').to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Write a body out without holding it, whichever side it is on.
async fn copy_file(body: &FileBody, path: &Path) -> io::Result<()> {
    match body {
        FileBody::Memory(bytes) => tokio::fs::write(path, bytes).await,
        FileBody::Spilled(file) => {
            let mut from = tokio::fs::File::open(file.path()?).await?;
            let mut to = tokio::fs::File::create(path).await?;
            tokio::io::copy(&mut from, &mut to).await?;
            to.flush().await
        }
    }
}

/// A file still arriving.
///
/// Where [`RpcFile`] is a part that has been taken off the wire and put
/// somewhere, this is the wire itself: the handler is handed the field while
/// the browser is still sending it, and whatever it does with the bytes —
/// hashing them, forwarding them to object storage, writing them out — happens
/// as they arrive. Nothing is spilled, because nothing is kept.
///
/// ```ignore
/// #[rpc]
/// pub async fn receive(note: String, upload: RpcUpload) -> Result<Stored> {
///     let key = format!("uploads/{}", upload.safe_name().unwrap_or_default());
///     let written = upload.save(&key).await?;
///     Ok(Stored { key, written })
/// }
/// ```
///
/// ## What it costs
///
/// Multipart is a sequential format: a field cannot be read before the ones
/// in front of it, and reading this one is what the handler is doing. So the
/// arguments beside it are the ones the browser sent *before* it, and a
/// payload that puts the file first leaves them unreachable — `#[rpc]` says
/// so at compile time by requiring the `RpcUpload` to be the last parameter,
/// and the call says so at runtime if the wire disagrees.
///
/// That is the trade. [`RpcFile`] costs a spill and has no such rule;
/// `RpcUpload` costs nothing and does. Reach for `RpcFile` unless the bytes
/// are going straight somewhere else.
pub struct RpcUpload {
    name: String,
    content_type: Option<String>,
    limit: usize,
    read: usize,
    field: multer::Field<'static>,
    /// The parser the field came out of. Not used again — held because
    /// dropping it early would be dropping the request body the field is
    /// still reading from.
    _parser: Box<multer::Multipart<'static>>,
}

impl RpcUpload {
    pub(crate) fn new(
        field: multer::Field<'static>,
        parser: Box<multer::Multipart<'static>>,
        limit: usize,
    ) -> Self {
        RpcUpload {
            name: field.file_name().unwrap_or_default().to_string(),
            content_type: field.content_type().map(|m| m.to_string()),
            limit,
            read: 0,
            field,
            _parser: parser,
        }
    }

    /// The name the browser reported. Input, like [`RpcFile::name`].
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The reported name reduced to something that cannot escape a directory.
    /// The same rule [`RpcFile::safe_name`] applies.
    pub fn safe_name(&self) -> Option<String> {
        safe_name(&self.name)
    }

    /// What the browser said this is — a hint, not a fact.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// How much has arrived so far.
    ///
    /// Not how large the file is: that is not known until it ends, which is
    /// the difference between this and [`RpcFile::len`].
    pub fn read_so_far(&self) -> usize {
        self.read
    }

    /// Lower the cap, so a test can reach it without a body the size of the
    /// real one.
    #[cfg(test)]
    pub(crate) fn set_limit_for_test(&mut self, limit: usize) {
        self.limit = limit;
    }

    /// The next piece, or `None` at the end of the file.
    pub async fn chunk(&mut self) -> crate::Result<Option<Bytes>> {
        let chunk = match self.field.chunk().await {
            Ok(chunk) => chunk,
            Err(e) => {
                return Err(Error::new(
                    StatusCode::BAD_REQUEST,
                    format!("rahti: `{}` did not arrive whole — {e}.", self.name),
                ));
            }
        };

        let Some(chunk) = chunk else { return Ok(None) };

        // Checked here rather than by a body limit alone, because the handler
        // is what is reading: without this a streaming rpc has no cap of its
        // own and a long body is a long write.
        self.read += chunk.len();
        if self.read > self.limit {
            return Err(Error::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "rahti: `{}` is larger than {} MiB.",
                    self.name,
                    self.limit / (1024 * 1024)
                ),
            ));
        }

        Ok(Some(chunk))
    }

    /// Write the file out as it arrives, and say how many bytes it was.
    ///
    /// Never holds more than the writer's buffer, whatever the file weighs.
    pub async fn save(mut self, path: impl AsRef<Path>) -> crate::Result<usize> {
        let file = tokio::fs::File::create(path.as_ref()).await?;
        let mut writer = tokio::io::BufWriter::new(file);

        while let Some(chunk) = self.chunk().await? {
            writer.write_all(&chunk).await?;
        }

        // A buffered writer dropped without this loses what is still in the
        // buffer, and does it silently.
        writer.flush().await?;
        writer.shutdown().await?;
        Ok(self.read)
    }

    /// Send the file into anything that takes bytes, as it arrives.
    ///
    /// The general form of [`RpcUpload::save`] — a hasher, a compressor, a
    /// socket.
    pub async fn copy_to<W>(mut self, writer: &mut W) -> crate::Result<usize>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        while let Some(chunk) = self.chunk().await? {
            writer.write_all(&chunk).await?;
        }
        writer.flush().await?;
        Ok(self.read)
    }

    /// Take the whole thing after all, as an [`RpcFile`].
    ///
    /// The escape hatch: a handler that turns out to need the bytes in hand
    /// spools them the ordinary way — in memory while it is small, on disk
    /// past `spill_at` — rather than being stuck with a stream it cannot
    /// rewind.
    pub async fn collect(mut self, spill_at: usize) -> crate::Result<RpcFile> {
        let mut spool = Spool::new(self.limit);
        while let Some(chunk) = self.chunk().await? {
            if let Err(e) = spool.push(chunk, spill_at).await {
                return Err(match e {
                    SpoolError::TooLarge(limit) => Error::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!("rahti: `{}` is larger than {limit} bytes.", self.name),
                    ),
                    SpoolError::Io(e) => Error::from(e),
                });
            }
        }

        let body = spool.finish().await.map_err(|e| match e {
            SpoolError::TooLarge(limit) => Error::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("rahti: `{}` is larger than {limit} bytes.", self.name),
            ),
            SpoolError::Io(e) => Error::from(e),
        })?;

        Ok(RpcFile {
            name: self.name,
            content_type: self.content_type,
            body,
        })
    }
}

impl std::fmt::Debug for RpcUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcUpload")
            .field("name", &self.name)
            .field("content_type", &self.content_type)
            .field("read_so_far", &self.read)
            .finish_non_exhaustive()
    }
}

/// Reads an [`RpcFile`], from wherever it turned out to live.
pub struct RpcFileReader(Source);

enum Source {
    Memory(io::Cursor<Bytes>),
    /// The handle, and the file it belongs to — held so that the temporary
    /// file outlives the reader rather than the other way round.
    File(tokio::fs::File, Arc<Spilled>),
}

impl AsyncRead for RpcFileReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Source::Memory(cursor) => Pin::new(cursor).poll_read(cx, buf),
            Source::File(handle, _) => Pin::new(handle).poll_read(cx, buf),
        }
    }
}

/// Collects one part as it arrives, in memory until that stops being sensible.
///
/// This is the piece that makes an upload's cost independent of its size: the
/// multipart loop pushes chunks in as they come off the socket and never
/// holds more than [`spill_threshold`] of any one part.
pub(crate) struct Spool {
    /// What a part may weigh before it is refused.
    limit: usize,
    /// Where it goes once it passes [`spill_threshold`]. `None` while it is
    /// still small enough to be held.
    file: Option<(PathBuf, tokio::io::BufWriter<tokio::fs::File>)>,
    held: Vec<u8>,
    len: usize,
}

/// Why a part could not be spooled.
pub(crate) enum SpoolError {
    /// Past the per-file limit. Carries the limit, so the message can say it.
    TooLarge(usize),
    Io(io::Error),
}

impl From<io::Error> for SpoolError {
    fn from(e: io::Error) -> Self {
        SpoolError::Io(e)
    }
}

impl Spool {
    /// One that refuses anything past `limit` bytes.
    pub(crate) fn new(limit: usize) -> Self {
        Spool {
            limit,
            file: None,
            held: Vec::new(),
            len: 0,
        }
    }

    /// Take the next chunk off the wire.
    ///
    /// `spill_at` is where holding stops. A part that is a value rather than
    /// a file passes `usize::MAX`: it becomes a string, so it is capped small
    /// and never goes to disk.
    pub(crate) async fn push(&mut self, chunk: Bytes, spill_at: usize) -> Result<(), SpoolError> {
        self.len += chunk.len();
        if self.len > self.limit {
            return Err(SpoolError::TooLarge(self.limit));
        }

        // Already on disk: keep going there.
        if let Some((_, writer)) = &mut self.file {
            writer.write_all(&chunk).await?;
            return Ok(());
        }

        // Still small enough to hold.
        if self.len <= spill_at {
            self.held.extend_from_slice(&chunk);
            return Ok(());
        }

        // The chunk that tipped it over: open the file, hand it what was
        // held, and carry on from there.
        let (path, file) = create_spill_file().await?;
        let mut writer = tokio::io::BufWriter::new(file);
        writer.write_all(&self.held).await?;
        writer.write_all(&chunk).await?;
        self.held = Vec::new();
        self.held.shrink_to_fit();
        self.file = Some((path, writer));
        Ok(())
    }

    /// How much has been taken so far.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Close the part off and say where it ended up.
    pub(crate) async fn finish(mut self) -> Result<FileBody, SpoolError> {
        match self.file.take() {
            None => Ok(FileBody::Memory(Bytes::from(self.held))),
            Some((path, mut writer)) => {
                // A buffered writer that is dropped without this loses
                // whatever was still in the buffer, silently.
                writer.flush().await?;
                writer.shutdown().await?;
                Ok(FileBody::Spilled(Arc::new(Spilled {
                    path: Some(path),
                    len: self.len,
                })))
            }
        }
    }

    /// The bytes as they stand, for a part that was never going to spill.
    pub(crate) fn into_memory(self) -> Vec<u8> {
        self.held
    }
}

/// Open a temporary file nobody else is going to pick.
///
/// The name is random rather than a counter because the spill directory is
/// shared with every other process on the machine, and `create_new` turns a
/// collision into an error rather than into two uploads writing over each
/// other. The randomness comes from the operating system, the same place
/// CSRF tokens come from.
async fn create_spill_file() -> io::Result<(PathBuf, tokio::fs::File)> {
    let dir = spill_dir();

    for _ in 0..8 {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|e| io::Error::other(format!("no entropy for a spill file name: {e}")))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("rahti-upload-{name}.part"));

        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }

    Err(io::Error::other(
        "rahti: could not open a file to spill an upload into",
    ))
}

/// Read a whole reader, for the tests and for callers that already decided
/// they want everything.
#[allow(dead_code)]
pub(crate) async fn read_all(mut reader: RpcFileReader) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out).await?;
    Ok(out)
}

#[cfg(test)]
#[path = "tests/upload.rs"]
mod tests;
