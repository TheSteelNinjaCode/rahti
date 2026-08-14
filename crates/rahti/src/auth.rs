//! Who is calling, and whether they may.
//!
//! Two things, and deliberately only two: a **session** the browser carries,
//! and a **policy** that says which URLs need one. Everything above that —
//! where users are stored, how a password is checked, what a "user" even is —
//! belongs to the application, the same way the database connection does.
//! Rahti never sees a password and has no user table.
//!
//! ## The session
//!
//! One cookie, signed and not encrypted. Its value is the JSON the application
//! handed to [`sign_in`] plus an expiry, and an HMAC-SHA256 tag over both:
//!
//! ```text
//! rahti_session = <base64url(payload)>.<base64url(tag)>
//! ```
//!
//! Nothing is stored on the server, so nothing has to be looked up: the check
//! is a hash over a few dozen bytes. The cost of that trade is the one every
//! stateless session pays — a cookie stays valid until it expires, because
//! there is no server-side record to revoke. Keep [`AuthSettings::validity`]
//! short if that matters, and note that the payload is *readable* by whoever
//! holds it. Signed means unforgeable, not secret: put an id and a display
//! name in there, not anything you would not show the user.
//!
//! ## The policy
//!
//! [`AuthSettings`] is app-owned and installed once from `main`, next to the
//! database connection:
//!
//! ```no_run
//! # use rahti::auth::AuthSettings;
//! rahti::auth::configure(AuthSettings {
//!     private_routes: vec!["/dashboard".into(), "/account".into()],
//!     ..AuthSettings::from_env()
//! });
//! ```
//!
//! Two ways round, and which one you want depends on the app. Most routes
//! public — list the few that are not, in `private_routes`. Most routes
//! private — set `all_private` and list the exceptions in `public_routes`.
//! Either way `auth_routes` is the sign-in page itself, which is the one place
//! an *already* authenticated visitor is sent away from.
//!
//! A route in any of those lists covers its subtree: `/account` protects
//! `/account/billing` too. Dynamic and catch-all segments are written the way
//! the file tree writes them — `/customers/[id]`, `/docs/[[...slug]]` — and the
//! axum forms (`/customers/{id}`, `/docs/{*slug}`) are understood as well, so
//! a path copied out of `src/routes.rs` works.
//!
//! ## Reaching it
//!
//! [`guard`] is a layer the generated router installs over every page, api
//! route and rpc. It decodes the cookie, applies the policy, and — on the way
//! back out — writes whatever [`sign_in`] or [`sign_out`] asked for. In
//! between, the session belongs to the task, so a page or an rpc reads it by
//! calling a function rather than by taking an argument:
//!
//! ```ignore
//! #[rpc(auth)]                       // 401 unless signed in
//! pub async fn rename(name: String) -> rahti::Result<Account> {
//!     let account: Account = rahti::auth::user().unwrap();
//!     …
//! }
//! ```
//!
//! `#[rpc(auth)]` is the whole of the rpc side. There are no roles here: a
//! call is either from someone signed in or it is not, and anything finer than
//! that is a question about *your* users, which the framework would only be
//! guessing at.

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::Sha256;

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;

/// The secret the cookie is signed with. A credential, so it lives in the
/// environment — `.env` is ignored by git for exactly this reason.
pub const SECRET_ENV: &str = "AUTH_SECRET";
/// The cookie's name. Written per project by `cargo rahti new`, because two
/// apps under one parent domain that share a cookie name overwrite each
/// other's sessions.
pub const COOKIE_ENV: &str = "AUTH_COOKIE_NAME";
/// How long a session lasts, in hours. Optional — the only one of the three
/// that is ordinary configuration rather than project identity.
pub const LIFETIME_ENV: &str = "SESSION_LIFETIME_HOURS";

/// The cookie's name when nothing says otherwise.
const DEFAULT_COOKIE: &str = "rahti_session";
/// How long a session lasts when `SESSION_LIFETIME_HOURS` says nothing.
const DEFAULT_LIFETIME_HOURS: u64 = 1;

/// Values that are not secrets, whatever file they are written in.
///
/// A scaffold writes a real key and an example file writes one of these, so
/// the difference between "configured" and "copied the template" is a thing
/// the server can actually see. Compared lowercased.
const PLACEHOLDERS: &[&str] = &[
    "change-me",
    "changeme",
    "change_me",
    "replace-me",
    "replaceme",
    "secret",
    "your-secret-here",
];
/// A cookie longer than this was not written here. Bounded so a hostile
/// `Cookie` header cannot make the server parse a megabyte of base64.
const MAX_COOKIE: usize = 8 * 1024;
/// The header `RpcClient` names the function in — how the guard tells an rpc
/// call from a navigation, and so a 401 from a redirect.
const FUNCTION_HEADER: &str = "x-pp-function";

// ---------------------------------------------------------------- settings

/// The application's auth policy.
///
/// Built with [`AuthSettings::from_env`] and adjusted with struct update
/// syntax, so a project writes down the decisions it actually made and
/// inherits the rest.
#[derive(Debug, Clone)]
pub struct AuthSettings {
    /// What the cookie is signed with. See [`AuthSettings::from_env`].
    pub secret: String,
    /// The cookie's name.
    pub cookie: String,
    /// How long a session lasts. Written into the payload, so an old cookie
    /// expires on the server's clock rather than on the browser's goodwill.
    pub validity: Duration,
    /// Re-issue the cookie on every authenticated request, which turns
    /// [`validity`](Self::validity) into an idle timeout rather than a hard
    /// one. Off by default: a sliding session is a choice, not a default.
    pub sliding: bool,
    /// Protect everything except [`public_routes`](Self::public_routes) and
    /// [`auth_routes`](Self::auth_routes).
    ///
    /// The right way round for an application that is mostly behind a login.
    /// For one that is mostly open, leave this off and list the exceptions in
    /// [`private_routes`](Self::private_routes).
    pub all_private: bool,
    /// The routes that need a session. Read only when
    /// [`all_private`](Self::all_private) is off.
    pub private_routes: Vec<String>,
    /// The routes that never need one. Read only when
    /// [`all_private`](Self::all_private) is on.
    pub public_routes: Vec<String>,
    /// Sign-in and sign-up: public, and redirected away from once the visitor
    /// already has a session.
    pub auth_routes: Vec<String>,
    /// Where an unauthenticated visitor to a private route is sent. The path
    /// they were after is appended as `?next=`.
    pub signin: String,
    /// Where they land after signing in, when the call does not say.
    pub after_signin: String,
    /// Where they land after signing out.
    pub after_signout: String,
}

impl Default for AuthSettings {
    fn default() -> Self {
        AuthSettings::from_env()
    }
}

impl AuthSettings {
    /// The defaults, with the three environment values read in.
    ///
    /// | Variable | Required | What it is |
    /// | --- | --- | --- |
    /// | `AUTH_SECRET` | in production | the signing key |
    /// | `AUTH_COOKIE_NAME` | no, but written per project | the cookie's name |
    /// | `SESSION_LIFETIME_HOURS` | no | how long a session lasts |
    ///
    /// `.env` is read first, and a variable already in the environment always
    /// wins over the file — see [`load_env`](crate::load_env).
    ///
    /// **`AUTH_SECRET` in production.** Missing, blank, or left on a
    /// placeholder is a refusal to start rather than a warning: a server that
    /// keeps going has just signed every session with a key that is either in
    /// somebody's git history or different on each instance, and it will look
    /// like it is working. In development the process invents one instead and
    /// says so, so a fresh clone runs with no setup — at the cost of everyone
    /// being signed out on restart, which is the right trade there and the
    /// wrong one in production.
    ///
    /// "Production" here is the absence of development mode: `RAHTI_DEV`, or a
    /// release build when it is unset. A release-mode test run therefore needs
    /// the variable set, the same as a deployment does.
    pub fn from_env() -> Self {
        AuthSettings::read_env(true)
    }

    /// The same, for a process that never asked for auth.
    ///
    /// Reached only through the lazily-initialized default in [`settings`] —
    /// an app that never calls [`configure`] has no policy, no private routes
    /// and no sessions, and must not be killed in production over a key it was
    /// never going to use.
    fn lenient() -> Self {
        AuthSettings::read_env(false)
    }

    fn read_env(strict: bool) -> Self {
        // Before any of the three are read, and idempotent — `db::connect`
        // calls it too, and neither has to know which of them ran first.
        super::env::load_env();

        AuthSettings {
            secret: secret_from_env(strict),
            cookie: cookie_from_env(strict),
            validity: validity_from_env(strict),
            sliding: false,
            all_private: false,
            private_routes: Vec::new(),
            public_routes: vec!["/".to_string()],
            auth_routes: vec!["/signin".to_string(), "/signup".to_string()],
            signin: "/signin".to_string(),
            after_signin: "/".to_string(),
            after_signout: "/".to_string(),
        }
    }

    /// Whether `path` is one of the sign-in pages.
    pub fn is_auth_route(&self, path: &str) -> bool {
        matches_any(&self.auth_routes, path)
    }

    /// Whether `path` is reachable without a session.
    pub fn is_public_route(&self, path: &str) -> bool {
        matches_any(&self.public_routes, path)
    }

    /// Whether `path` needs one.
    ///
    /// The sign-in pages are never private, whichever way round the policy is
    /// — a login you have to be logged in to reach is a locked-out app.
    pub fn is_private_route(&self, path: &str) -> bool {
        if self.is_auth_route(path) {
            return false;
        }
        if self.all_private {
            return !self.is_public_route(path);
        }
        matches_any(&self.private_routes, path)
    }
}

/// Why a configured secret is not usable, when it is not.
///
/// Separate from reading it so the rule can be tested without an environment
/// variable and without a process that exits.
#[derive(Debug, PartialEq)]
enum SecretProblem {
    /// Unset, or nothing but whitespace.
    Missing,
    /// One of [`PLACEHOLDERS`] — the value an example file carries.
    Placeholder,
    /// Short enough to be worth guessing. An HMAC key is only as good as its
    /// entropy, and "password" is a valid `String`.
    TooShort,
}

/// The floor a signing key has to clear.
///
/// Not a policy about passwords — nobody types this — so it is only here to
/// catch a hand-written value that was never going to be a key.
const MIN_SECRET: usize = 16;

fn check_secret(raw: &str) -> Result<(), SecretProblem> {
    let secret = raw.trim();
    if secret.is_empty() {
        return Err(SecretProblem::Missing);
    }
    if PLACEHOLDERS.contains(&secret.to_ascii_lowercase().as_str()) {
        return Err(SecretProblem::Placeholder);
    }
    if secret.len() < MIN_SECRET {
        return Err(SecretProblem::TooShort);
    }
    Ok(())
}

fn secret_from_env(strict: bool) -> String {
    let configured = std::env::var(SECRET_ENV).unwrap_or_default();

    let problem = match check_secret(&configured) {
        Ok(()) => return configured,
        Err(problem) => problem,
    };

    // Development invents one. Production does not: a server that keeps going
    // here has signed every session with a key that is either public or
    // per-instance, and nothing about it looks broken until it is.
    if strict && !super::dev::enabled() {
        let detail = match problem {
            SecretProblem::Missing => format!("{SECRET_ENV} is not set."),
            SecretProblem::Placeholder => {
                format!("{SECRET_ENV} is still the placeholder value from `.env.example`.")
            }
            SecretProblem::TooShort => {
                format!("{SECRET_ENV} is shorter than {MIN_SECRET} characters, which is not a key.")
            }
        };
        eprintln!(
            "rahti: {detail}\n  \
             It signs every session cookie, so it is a credential: put a long \
             random value in `.env`, which git ignores.\n  \
             `cargo rahti new` writes one per project — copy that line, or \
             generate one:\n    \
             openssl rand -base64 32\n  \
             Development invents a key instead; this is a release build, where \
             guessing would be worse than stopping."
        );
        std::process::exit(1);
    }

    // Once per process, not once per call: a secret that changed between two
    // requests would invalidate the cookie it had just written.
    static GENERATED: OnceLock<String> = OnceLock::new();
    GENERATED
        .get_or_init(|| {
            let secret = random_hex(32);
            let detail = match problem {
                SecretProblem::Missing => "is not set",
                SecretProblem::Placeholder => "is still a placeholder",
                SecretProblem::TooShort => "is too short to be a key",
            };
            super::dev::record_server(
                "warn",
                "auth",
                &format!(
                    "rahti: {SECRET_ENV} {detail}, so sessions are signed with a key \
                     this process invented.\n  \
                     Everyone is signed out when it restarts, and a second instance \
                     will not accept its cookies. Put the value `cargo rahti new` \
                     wrote into `.env` — a release build refuses to start without it."
                ),
            );
            secret
        })
        .clone()
}

/// The cookie's name.
///
/// Defaulted rather than required: an app that runs alone is fine sharing the
/// framework's name with every other Rahti app on a different host. What it is
/// *not* fine to share is a parent domain — `a.example.com` and
/// `b.example.com` both write to `.example.com`, and one session would
/// overwrite the other. `cargo rahti new` writes a per-project value so that
/// case is already handled before anyone runs into it.
fn cookie_from_env(strict: bool) -> String {
    let configured = std::env::var(COOKIE_ENV).unwrap_or_default();
    let configured = configured.trim();

    if configured.is_empty() {
        return DEFAULT_COOKIE.to_string();
    }

    if check_cookie_name(configured).is_ok() {
        return configured.to_string();
    }

    // Refused rather than passed through, because both ways it fails are
    // silent. `read_cookie` splits a `Cookie` header on `;` and `=`, so a name
    // carrying either can never be matched back — the cookie is written, never
    // read, and every sign-in appears to work while nobody stays signed in.
    // A `;` additionally injects whatever follows as a cookie attribute.
    let message = format!(
        "rahti: {COOKIE_ENV} is `{configured}`, which is not a legal cookie name.\n  \
         It has to be a token: letters, digits, and `-_.~!#$%&'*+^|` — no \
         spaces, and none of `;` `=` `,` `\"` `\\` `@` `:` `/` `?` `(` `)` \
         `<` `>` `[` `]` `{{` `}}`.\n  \
         `cargo rahti new` writes a hex value; anything you would put in a \
         URL path segment is fine."
    );

    if strict {
        eprintln!("{message}");
        std::process::exit(1);
    }
    super::dev::record_server("warn", "auth", &message);
    DEFAULT_COOKIE.to_string()
}

/// An RFC 6265 cookie name, which is an RFC 7230 `token`.
///
/// Checked rather than escaped: there is no escaping for a cookie name, and a
/// name nobody can read back is not a thing to quietly repair.
fn check_cookie_name(name: &str) -> Result<(), ()> {
    if name.is_empty() {
        return Err(());
    }

    // A token is any visible ASCII except the separators. Written as the
    // allowed set rather than the forbidden one so that a character nobody
    // thought about — a control, anything non-ASCII — is refused by default.
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);

    if name.chars().all(token) {
        Ok(())
    } else {
        Err(())
    }
}

/// `SESSION_LIFETIME_HOURS`, or one hour.
///
/// A value that is present but unreadable is refused rather than defaulted,
/// in both modes. The two silent outcomes are both wrong: `SESSION_LIFETIME_HOURS=24h`
/// falling back to one hour signs people out all day for no visible reason,
/// and there is no environment in which a typo here is what someone meant.
fn validity_from_env(strict: bool) -> Duration {
    let Ok(raw) = std::env::var(LIFETIME_ENV) else {
        return Duration::from_secs(DEFAULT_LIFETIME_HOURS * 60 * 60);
    };

    let raw = raw.trim();
    if raw.is_empty() {
        return Duration::from_secs(DEFAULT_LIFETIME_HOURS * 60 * 60);
    }

    match parse_hours(raw) {
        Some(hours) => Duration::from_secs(hours * 60 * 60),
        None => {
            let message = format!(
                "rahti: {LIFETIME_ENV} is `{raw}`, which is not a number of hours.\n  \
                 It is a whole number greater than zero — `SESSION_LIFETIME_HOURS=7` \
                 is a seven-hour session. Remove it for the default of \
                 {DEFAULT_LIFETIME_HOURS}."
            );
            if strict {
                eprintln!("{message}");
                std::process::exit(1);
            }
            super::dev::record_server("warn", "auth", &message);
            Duration::from_secs(DEFAULT_LIFETIME_HOURS * 60 * 60)
        }
    }
}

/// Whole hours, greater than zero, and small enough to stay a duration.
fn parse_hours(raw: &str) -> Option<u64> {
    let hours: u64 = raw.parse().ok()?;
    // A zero-hour session expires before the response carrying it arrives, so
    // it is a mistake rather than a way to disable sessions.
    (hours > 0 && hours <= 24 * 365 * 10).then_some(hours)
}

/// The installed policy.
///
/// A `RwLock` rather than a `OnceLock` because a test suite configures more
/// than one policy in a process, and reading it is not on any hot path — the
/// guard takes it once per request.
fn cell() -> &'static RwLock<Arc<AuthSettings>> {
    static CELL: OnceLock<RwLock<Arc<AuthSettings>>> = OnceLock::new();
    // `lenient`, not `from_env`: reaching this means the app never called
    // `configure`, so it has no private routes and no sessions — and killing
    // such a process in production over an unset key would be refusing to
    // start over a feature it does not use.
    CELL.get_or_init(|| RwLock::new(Arc::new(AuthSettings::lenient())))
}

/// Install the policy. Called once from `main`, before the listener binds.
pub fn configure(settings: AuthSettings) {
    match cell().write() {
        Ok(mut slot) => *slot = Arc::new(settings),
        Err(poisoned) => *poisoned.into_inner() = Arc::new(settings),
    }
}

/// The policy in force.
pub fn settings() -> Arc<AuthSettings> {
    match cell().read() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

// ----------------------------------------------------------------- session

/// What the browser is carrying.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Whatever the application signed in — its shape is the app's business.
    user: Value,
    /// Unix seconds. Checked here, so a browser that ignores `Max-Age` gains
    /// nothing.
    expires: u64,
}

impl Session {
    /// The payload as it was signed in.
    pub fn user(&self) -> &Value {
        &self.user
    }

    /// The payload, as the type the application put in.
    pub fn user_as<T: DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.user.clone()).ok()
    }

    /// When it stops being valid, in unix seconds.
    pub fn expires_at(&self) -> u64 {
        self.expires
    }

    fn expired(&self) -> bool {
        now() >= self.expires
    }
}

/// What the guard has to do to the cookie on the way out.
#[derive(Debug, Clone)]
enum Pending {
    Set(Session),
    Clear,
}

/// The session for the request being served.
///
/// Held in the task rather than passed as an argument, for the same reason
/// `db()` is a function: a Rahti handler is an ordinary Rust function and takes
/// no framework state.
#[derive(Debug, Default)]
struct Context {
    session: Mutex<Option<Session>>,
    pending: Mutex<Option<Pending>>,
}

tokio::task_local! {
    static CONTEXT: Arc<Context>;
}

/// Run `f` against the current request's context, if there is one.
///
/// There is not, outside a request — a unit test, or the 404 handler, which
/// sits below the router's fallback rather than inside it. Reading the session
/// there answers "nobody" rather than panicking: a page that renders for an
/// anonymous visitor should also render when asked outside a request at all.
fn with_context<T>(f: impl FnOnce(&Context) -> T) -> Option<T> {
    CONTEXT.try_with(|ctx| f(ctx)).ok()
}

/// The same, for the two calls that *change* something.
///
/// Reading outside a request is an ordinary answer — nobody is signed in — but
/// writing outside one is a call that cannot do what it says. The cookie is
/// written by [`guard`] as the response leaves, so with no guard above the
/// caller there is no response for it to be written to: [`sign_in`] would
/// return normally and the visitor would still be anonymous.
///
/// Reported rather than silently dropped, because the symptom otherwise is a
/// sign-in that reports success and does nothing. The two ways to reach it are
/// a handler below the router's fallback — the root `not-found.rs` — and code
/// running outside a request altogether, such as a spawned task.
fn with_context_mut(what: &str, f: impl FnOnce(&Context)) {
    if with_context(f).is_none() {
        super::dev::record_server(
            "error",
            "auth",
            &format!(
                "rahti: `auth::{what}` was called outside a request, so it did \
                 nothing.\n  \
                 The session cookie is written by the auth layer as a response \
                 leaves, and that layer covers pages, `route.rs` handlers and \
                 rpcs — but not the root `not-found.rs`, which is below the \
                 router's fallback, and not a spawned task, which outlives the \
                 request that started it.\n  \
                 Call it from the handler itself."
            ),
        );
    }
}

/// Run `fut` with `session` readable through [`session`].
///
/// A WebSocket handler runs after the upgrade, outside the request that
/// carried the cookie — the guard's task-local is gone by then. The socket
/// endpoint reads the session while the handshake is still a request and
/// scopes it back in around the handler, so `auth::session()` answers there
/// the way it does in an rpc. Read-only on purpose: there is no response
/// left for `sign_in`/`sign_out` to write a cookie onto, and the context's
/// `pending` half is simply never read again.
#[cfg(feature = "ws")]
pub(crate) async fn scope_session<F: std::future::Future>(
    session: Option<Session>,
    fut: F,
) -> F::Output {
    let context = Arc::new(Context {
        session: Mutex::new(session),
        pending: Mutex::new(None),
    });
    CONTEXT.scope(context, fut).await
}

/// The current session, or `None` for an anonymous request.
pub fn session() -> Option<Session> {
    with_context(|ctx| ctx.session.lock().ok().and_then(|s| s.clone())).flatten()
}

/// Whether this request carries a valid session.
pub fn is_authenticated() -> bool {
    session().is_some()
}

/// The signed-in user, as the type the application signed in.
///
/// ```ignore
/// #[derive(Serialize, Deserialize)]
/// struct Account { id: i32, email: String }
///
/// if let Some(account) = rahti::auth::user::<Account>() { … }
/// ```
///
/// `None` covers both "nobody is signed in" and "the payload is not that
/// type", which is the same answer for a caller: there is no such user here.
pub fn user<T: DeserializeOwned>() -> Option<T> {
    session().and_then(|s| s.user_as())
}

/// Start a session for `user`.
///
/// The cookie is written by [`guard`] as the response leaves, so this is
/// callable from anywhere inside a request — a page, an rpc, a `route.rs`
/// handler — and the caller returns whatever it was going to return.
///
/// The payload is signed, not encrypted. Sign in an id and the handful of
/// fields the UI needs; look the rest up.
pub fn sign_in<T: Serialize>(user: &T) {
    let Ok(value) = serde_json::to_value(user) else {
        super::dev::record_server(
            "error",
            "auth",
            "rahti: the value passed to `sign_in` cannot be written as JSON, \
             so no session was started.",
        );
        return;
    };

    let session = Session {
        user: value,
        expires: now() + settings().validity.as_secs(),
    };

    with_context_mut("sign_in", |ctx| {
        if let Ok(mut slot) = ctx.session.lock() {
            *slot = Some(session.clone());
        }
        if let Ok(mut slot) = ctx.pending.lock() {
            *slot = Some(Pending::Set(session));
        }
    });
}

/// End it. The cookie is cleared as the response leaves.
pub fn sign_out() {
    with_context_mut("sign_out", |ctx| {
        if let Ok(mut slot) = ctx.session.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = ctx.pending.lock() {
            *slot = Some(Pending::Clear);
        }
    });
}

/// Refuse the call unless someone is signed in.
///
/// What `#[rpc(auth)]` expands to, and callable by hand from a page or a
/// `route.rs` that wants the same check. The `Err` side is the finished
/// response — `{"error": …}` with a 401, which is the shape `pp.rpc` reads
/// before it throws.
#[allow(clippy::result_large_err)]
pub fn require() -> Result<Session, Response> {
    session().ok_or_else(|| {
        super::rpc::json_error(
            StatusCode::UNAUTHORIZED,
            "rahti: this call needs a signed-in session.\n  \
             The function is `#[rpc(auth)]`, so it answers only while the \
             browser is carrying one — sign in, or drop the `auth` if the \
             call is meant to be open.",
        )
    })
}

// ------------------------------------------------------------------- guard

/// Decode the session, apply the policy, write the cookie.
///
/// Installed by the generated router over every page group, every `route.rs`,
/// and the rpc router — so it covers everything the app answers and nothing
/// under `public/`, which needs no session to be served.
///
/// An app that never calls [`configure`] gets the default policy, under which
/// no route is private: the layer then costs one absent-cookie lookup and
/// changes nothing.
pub async fn guard(req: Request, next: Next) -> Response {
    let settings = settings();
    let path = req.uri().path().to_string();
    let is_rpc = req.headers().get(FUNCTION_HEADER).is_some();

    let session = read_cookie(req.headers(), &settings.cookie)
        .and_then(|raw| decode(&raw, &settings.secret))
        .filter(|session| !session.expired());

    // A call from `pp.rpc` is read by `fetch`, which follows a redirect and
    // hands the page's HTML to a caller expecting JSON. Say what happened
    // instead. The rpc's own `#[rpc(auth)]` is the finer check; this is the
    // one that covers a page-wide policy.
    if is_rpc {
        if session.is_none() && settings.is_private_route(&path) {
            return super::rpc::json_error(
                StatusCode::UNAUTHORIZED,
                format!(
                    "rahti: `{path}` is a private route and this call carried no \
                     session.\n  \
                     Sign in first — the rpc is reached from a page the visitor \
                     cannot currently load."
                ),
            );
        }
    } else if session.is_some() && settings.is_auth_route(&path) {
        // Already signed in, and asking for the sign-in page. `next=` is
        // honoured here so a visitor who was bounced to `/signin`, and turns
        // out to have a session already, still lands where they were going.
        let target = query_param(req.uri().query(), "next")
            .map(|next| safe_target(&next, &settings.after_signin))
            .unwrap_or_else(|| safe_target(&settings.after_signin, "/"));
        return redirect(&target);
    } else if session.is_none() && settings.is_private_route(&path) {
        let signin = safe_target(&settings.signin, "/");
        return redirect(&format!("{signin}?next={}", encode_query(&path)));
    }

    let context = Arc::new(Context {
        session: Mutex::new(session.clone()),
        pending: Mutex::new(None),
    });

    // A sliding session is re-issued before the handler runs rather than
    // after, so a handler that signs out still wins: `sign_out` overwrites the
    // pending slot.
    if settings.sliding
        && let Some(current) = session
    {
        let refreshed = Session {
            expires: now() + settings.validity.as_secs(),
            ..current
        };
        if let Ok(mut slot) = context.session.lock() {
            *slot = Some(refreshed.clone());
        }
        if let Ok(mut slot) = context.pending.lock() {
            *slot = Some(Pending::Set(refreshed));
        }
    }

    let mut res = CONTEXT.scope(context.clone(), next.run(req)).await;

    let pending = context.pending.lock().ok().and_then(|slot| slot.clone());
    match pending {
        Some(Pending::Set(session)) => {
            if let Some(value) = encode(&session, &settings.secret) {
                set_cookie(
                    &mut res,
                    &cookie_line(&settings.cookie, &value, session.expires),
                );
            }
        }
        Some(Pending::Clear) => {
            set_cookie(&mut res, &cleared_cookie_line(&settings.cookie));
        }
        None => {}
    }

    res
}

/// `Set-Cookie`, appended rather than inserted: the CSRF layer sets one too.
fn set_cookie(res: &mut Response, line: &str) {
    if let Ok(value) = HeaderValue::from_str(line) {
        res.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// `HttpOnly`, unlike the CSRF cookie: nothing in the browser needs to read
/// this one, and a script that could read it could steal the session.
///
/// `SameSite=Lax` still arrives on the top-level navigation that renders a
/// page, which is when the session has to be known. `Secure` is set outside
/// development, where the site is not on plain http.
fn cookie_line(name: &str, value: &str, expires: u64) -> String {
    let max_age = expires.saturating_sub(now());
    let mut line = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if !super::dev::enabled() {
        line.push_str("; Secure");
    }
    line
}

fn cleared_cookie_line(name: &str) -> String {
    let mut line = format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if !super::dev::enabled() {
        line.push_str("; Secure");
    }
    line
}

fn redirect(target: &str) -> Response {
    // 303, so a redirect out of a POST is followed as a GET.
    match HeaderValue::from_str(target) {
        Ok(value) => {
            let mut res = StatusCode::SEE_OTHER.into_response();
            res.headers_mut().insert(header::LOCATION, value);
            res
        }
        Err(_) => StatusCode::SEE_OTHER.into_response(),
    }
}

// ------------------------------------------------------------------ cookie

/// One cookie out of the `Cookie` header, by name.
fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty() && value.len() <= MAX_COOKIE)
}

/// `<payload>.<tag>`, both base64url without padding.
fn encode(session: &Session, secret: &str) -> Option<String> {
    let payload = serde_json::json!({ "user": session.user, "exp": session.expires });
    let body = B64.encode(serde_json::to_vec(&payload).ok()?);
    let tag = B64.encode(tag(body.as_bytes(), secret)?);
    Some(format!("{body}.{tag}"))
}

/// The reverse, and the only place a cookie becomes a session.
///
/// Every failure is the same answer — `None`, an anonymous request. A cookie
/// that does not verify is not a case to report to whoever sent it.
fn decode(raw: &str, secret: &str) -> Option<Session> {
    let (body, sent) = raw.split_once('.')?;
    let sent = B64.decode(sent).ok()?;

    // Constant-time, via `Mac::verify_slice`: comparing tags with `==` leaks
    // how many leading bytes were right, which is enough to forge one.
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(body.as_bytes());
    mac.verify_slice(&sent).ok()?;

    let payload: Value = serde_json::from_slice(&B64.decode(body).ok()?).ok()?;
    Some(Session {
        user: payload.get("user")?.clone(),
        expires: payload.get("exp")?.as_u64()?,
    })
}

fn tag(body: &[u8], secret: &str) -> Option<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(body);
    Some(mac.finalize().into_bytes().to_vec())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Random hex, asked of the operating system rather than the clock.
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if getrandom::fill(&mut buf).is_err() {
        // Only reachable if the OS has no entropy source at all. A constant
        // would be worse than a process that refuses to pretend it has a key.
        panic!("rahti: cannot read random bytes for a session key");
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------- paths

/// `/a/b/` and `a/b` both become `/a/b`; an empty path becomes `/`.
fn normalize(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "/".to_string();
    }
    let mut out = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

fn segments(path: &str) -> Vec<String> {
    normalize(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// How many path segments one pattern segment stands for.
enum Segment {
    /// A literal, or a single dynamic segment: `docs`, `[id]`, `{id}`.
    One,
    /// One or more: `[...id]`, `{*id}`.
    Rest,
    /// Zero or more: `[[...slug]]`.
    Maybe,
}

fn classify(segment: &str) -> (Segment, bool) {
    let dynamic = |s: &str| {
        (s.starts_with('[') && s.ends_with(']')) || (s.starts_with('{') && s.ends_with('}'))
    };

    if segment.starts_with("[[...") && segment.ends_with("]]") {
        (Segment::Maybe, true)
    } else if (segment.starts_with("[...") && segment.ends_with(']'))
        || (segment.starts_with("{*") && segment.ends_with('}'))
    {
        (Segment::Rest, true)
    } else if dynamic(segment) {
        (Segment::One, true)
    } else {
        (Segment::One, false)
    }
}

/// Whether `pattern` matches `path` exactly, segment for segment.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = segments(pattern);
    let path = segments(path);

    let mut p = 0;
    let mut i = 0;
    while p < pattern.len() {
        let (kind, dynamic) = classify(&pattern[p]);
        match kind {
            // A catch-all is last by construction, so it takes the remainder.
            Segment::Rest => return path.len() > i,
            Segment::Maybe => return true,
            Segment::One => {
                if i >= path.len() {
                    return false;
                }
                if !dynamic && pattern[p] != path[i] {
                    return false;
                }
                p += 1;
                i += 1;
            }
        }
    }
    p == pattern.len() && i == path.len()
}

/// Whether `scope` covers `path` — the pattern itself, or anything under it.
///
/// `/account` covers `/account/billing`, which is what makes a policy a
/// handful of lines rather than one line per URL. `/` is the exception: it
/// covers only itself, or listing it as public would make everything public.
fn scope_matches(scope: &str, path: &str) -> bool {
    if pattern_matches(scope, path) {
        return true;
    }

    let scope_segments = segments(scope);
    let path_segments = segments(path);
    if scope_segments.is_empty() || path_segments.len() <= scope_segments.len() {
        return false;
    }

    let prefix = format!("/{}", path_segments[..scope_segments.len()].join("/"));
    pattern_matches(scope, &prefix)
}

fn matches_any(scopes: &[String], path: &str) -> bool {
    scopes.iter().any(|scope| scope_matches(scope, path))
}

/// A redirect target that cannot leave this origin.
///
/// `next=` arrives in a URL anyone can write, so it is the classic open
/// redirect: a scheme, a host, or a protocol-relative `//evil.test` is
/// discarded in favour of the fallback rather than followed.
fn safe_target(target: &str, fallback: &str) -> String {
    let fallback = normalize(fallback);
    let target = target.trim();

    if target.is_empty() || target.starts_with("//") || target.starts_with('\\') {
        return fallback;
    }
    // A scheme, or a host — anything before the path that is not the path.
    if target.contains("://") || !target.starts_with('/') {
        return fallback;
    }
    // `%2f%2f`, and the rest of the encoded ways to write a host. The path is
    // ours to normalize, so an escape that survives decoding is refused here.
    if target.contains('\\') || target.to_ascii_lowercase().contains("%2f%2f") {
        return fallback;
    }

    let (path, rest) = match target.find(['?', '#']) {
        Some(at) => (&target[..at], &target[at..]),
        None => (target, ""),
    };
    format!("{}{rest}", normalize(path))
}

/// One query parameter, by name, percent-decoded.
fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode_query(value))
}

/// Percent-encode a path so it survives being a query value.
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode_query(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
