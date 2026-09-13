# Authentication

Rahti provides two things and no more: a **signed session cookie**, and a
**policy** that says which URLs require one. Users, passwords, registration,
password resets, and anything resembling a role or permission belong to the
application.

There is deliberately no OAuth provider, no user table, and no role system in
the framework. `rahti` has no database dependency and never sees a password.

## Ownership

| Piece | Owner |
| --- | --- |
| Session cookie, encode/decode, expiry | `rahti::auth` |
| Route guard and redirects | `rahti::auth::guard`, installed by generated code |
| `#[rpc(auth)]` | `rahti-macros` |
| Which routes are private | the application, in `src/auth.rs` |
| Users, hashing, sign-up rules | the application |

The application half is `src/auth.rs` — the policy and the password check —
plus whatever the application builds around it: a `user` entity and its
migration, a sign-in page, and the private pages the policy protects.

## The session

One cookie:

```text
rahti_session = <base64url(payload)>.<base64url(hmac-sha256(payload))>
```

The payload is `{"user": <whatever you signed in>, "exp": <unix seconds>}`.

- **Signed, not encrypted.** Anyone holding the cookie can read the payload.
  Put an id and the handful of fields the UI needs in it. Never a hash, a
  token, an email verification state you rely on, or anything you would not
  show the user.
- **Stateless.** Nothing is stored server-side, so nothing can be revoked. A
  cookie is valid until it expires. Keep `validity` short if that matters.
- **`HttpOnly`, `SameSite=Lax`, `Path=/`,** and `Secure` outside development.
  Unlike the CSRF cookie, nothing in the browser reads this one.
- **Expiry is checked on the server** from the payload, not from `Max-Age`.

## Environment

Three variables, read by `rahti::auth` from the environment or from `.env`. A
real environment variable always wins over the file.

| Variable | Required | Default | What it is |
| --- | --- | --- | --- |
| `AUTH_SECRET` | in production | a per-process random key | the HMAC signing key |
| `AUTH_COOKIE_NAME` | no | `rahti_session` | the cookie's name |
| `SESSION_LIFETIME_HOURS` | no | `1` | how long a session lasts |

`cargo rahti new` generates the first two per project and writes them to
`.env`, so no two Rahti applications share a signing key or a cookie name. It
writes them whether or not the project has a database — `AUTH_SECRET` is not a
database setting.

### `AUTH_SECRET`

**In production, missing or placeholder means the server refuses to start.**
Not a warning: a process that keeps going has signed every session with a key
that is either in somebody's git history or different on each instance, and
nothing about it looks broken until it is. `change-me`, `changeme`, `secret`
and friends are recognized as placeholders, and anything under 16 characters
is rejected as too short to be a key.

**In development it falls back**, inventing a key and warning. A fresh clone
runs with no setup; the cost is that everyone is signed out on restart and a
second instance rejects the first's cookies.

"Production" is the absence of development mode: `RAHTI_DEV`, or a release
build when it is unset. A release-mode test run therefore needs the variable
set, the same as a deployment does.

The refusal only applies to an application that asked for auth by calling
`configure(AuthSettings::from_env())`. A project that never configures a policy
has no private routes and no sessions, and is not killed over a key it was
never going to use.

Generate one with:

```bash
openssl rand -base64 32
```

### `AUTH_COOKIE_NAME`

Validated as an RFC 6265 token: letters, digits, and ``-_.~!#$%&'*+^|` ``.
Anything else is refused at startup rather than passed through, because both
ways it fails are silent. A `Cookie` header is split on `;` and `=`, so a name
carrying either is written but can never be read back — every sign-in appears
to succeed and nobody stays signed in — and a `;` additionally injects whatever
follows it as a cookie attribute.

Not a credential — it is the project's identity under a shared parent domain.
`a.example.com` and `b.example.com` both write cookies to `.example.com`, so
two apps sharing a cookie name overwrite each other's sessions. Generated per
project so that case is handled before anyone runs into it, and the same name
has to be used everywhere that project runs: it is one value the deployment
copies from development rather than invents.

### `SESSION_LIFETIME_HOURS`

The only one of the three that is ordinary configuration. A whole number of
hours, greater than zero. A value that is present but unreadable —
`SESSION_LIFETIME_HOURS=24h` — is refused rather than defaulted: falling back
to one hour would sign people out all day for no visible reason, and there is
no environment in which a typo here is what someone meant.

### `.env`

`cargo rahti new` writes one `.env`, with this project's generated key and
cookie name in it, and git ignores it. Nothing is committed in its place — a
signing secret in git is not a secret. The three variables above are the whole
list, and the reference for them is this section and the environment reference
at <https://rahti.tsnc.tech/docs/environment>.

That means a fresh clone has no `.env` at all. In development it still runs —
`AUTH_SECRET` is invented and warned about — and before it is deployed the
three values are written by hand or set in the environment. `AUTH_COOKIE_NAME`
is the one that has to be carried over rather than made up again.

`.env` is not tracked in the scaffold ledger, so `cargo rahti upgrade` never
rewrites it — it only ever adds a `DATABASE_URL` above what is already there,
and only when the project gains a database. That is what protects your
credentials from an upgrade.

`.env` is read by `rahti::load_env()`, which both `AuthSettings::from_env` and
the application's `db::connect` call. It is idempotent, so neither has to know
which ran first, and a project without a database still reaches its variables.

## The policy

Built in `src/auth.rs` and installed from `main` before the listener binds:

```rust
rahti::auth::configure(auth::settings());
```

`AuthSettings::from_env()` supplies the defaults; name only what differs.

```rust
pub fn settings() -> AuthSettings {
    AuthSettings {
        // Fixed lifetime. Set true to refresh the expiry on each
        // authenticated request and make it an idle timeout.
        sliding: false,
        private_routes: vec!["/account".to_string()],
        auth_routes: vec!["/signin".to_string()],
        after_signin: "/account".to_string(),
        after_signout: "/".to_string(),
        signin: "/signin".to_string(),
        ..AuthSettings::from_env()
    }
}
```

Keep `sliding` explicit in the application policy: `false` gives every
session the fixed lifetime from `SESSION_LIFETIME_HOURS`; `true` makes that
lifetime an idle timeout. The guard performs the refresh automatically on
authenticated requests — application handlers do not call a separate refresh
function.

| Field | Meaning |
| --- | --- |
| `secret` | HMAC key. `AUTH_SECRET`, or a per-process random one. |
| `cookie` | Cookie name. `AUTH_COOKIE_NAME`, default `rahti_session`. |
| `validity` | Session lifetime. `SESSION_LIFETIME_HOURS`, default one hour. |
| `sliding` | Re-issue on every authenticated request, making `validity` an idle timeout. Off by default. |
| `all_private` | Protect everything except `public_routes` and `auth_routes`. |
| `private_routes` | The routes that need a session. Read only when `all_private` is off. |
| `public_routes` | The exceptions. Read only when `all_private` is on. |
| `auth_routes` | Sign-in and sign-up: public, and redirected away from once signed in. |
| `signin` | Where a signed-out visitor to a private route is sent, with `?next=`. |
| `after_signin` | Default landing route. |
| `after_signout` | Where signing out lands. |

Two ways round, and the choice is the application's:

- **Mostly public** — leave `all_private` off and list `private_routes`.
- **Mostly private** — set `all_private` and list `public_routes`.

`auth_routes` is never private under either mode. A login you must be logged in
to reach is a locked-out application.

### How a route is matched

A listed route **covers its subtree**: `/account` protects `/account/billing`
and everything below it. `/` is the exception — it covers only itself, or
listing it as public would make the whole application public.

Segments may be written either the way the file tree writes them or the way
`src/routes.rs` writes them:

| Meaning | File-tree form | Axum form |
| --- | --- | --- |
| One dynamic segment | `/customers/[id]` | `/customers/{id}` |
| One or more | `/posts/[...id]` | `/posts/{*id}` |
| Zero or more | `/docs/[[...slug]]` | — |

A trailing slash is not a different route.

## The guard

`rahti::auth_guard` is installed by generated code as the outermost layer of
every page group, every `route.rs` group, and the rpc router. It is not
installed over `public/`: a stylesheet needs no session, and an all-private
application whose stylesheet redirected to the sign-in page would render that
page unstyled.

The layer is always generated. An application that never calls `configure` gets
the default policy, under which nothing is private, and pays one absent-cookie
lookup.

Per request, in order:

1. Decode and verify the cookie; discard it if expired.
2. **RPC calls** (a POST carrying `X-PP-Function`) on a private route with no
   session get `401` with `{"error": …}` — never a redirect, because `fetch`
   would follow it and hand the sign-in page's HTML to a caller expecting JSON.
3. **Navigations** to an auth route while signed in redirect to `?next=`, or to
   `after_signin`.
4. **Navigations** to a private route with no session redirect to
   `<signin>?next=<path>` with `303`.
5. Otherwise the handler runs, inside a task-local session scope.
6. On the way out, whatever `sign_in` or `sign_out` asked for is written as a
   `Set-Cookie`.

## Reading the session

Handlers take no framework state, so the session is reached by calling a
function — the same shape as `crate::db::db()`.

```rust
rahti::auth::is_authenticated() -> bool
rahti::auth::session()          -> Option<Session>
rahti::auth::user::<T>()        -> Option<T>   // T: DeserializeOwned
rahti::auth::sign_in(&value)                   // value: Serialize
rahti::auth::sign_out()
rahti::auth::require()          -> Result<Session, Response>
```

`sign_in` and `sign_out` do not write headers. They record what the guard
should do, and the guard does it as the response leaves — so they are callable
from a page, an rpc, or a `route.rs` handler, and the caller returns whatever
it was going to return.

Outside a request — a unit test, or the root `not-found.rs`, which sits below
the router's fallback rather than inside it — the session **reads** as `None`
rather than panicking. **Writing** there is reported as an error to
`.rahti/dev.log`: with no guard above the caller there is no response for the
cookie to be written to, so `sign_in` would return normally and leave the
visitor anonymous. The same applies to a spawned task, which outlives the
request that started it — call `sign_in` from the handler, not from work it
kicked off.

### Read the installed policy, not a rebuilt one

`rahti::auth::settings()` returns the policy `configure` installed. The
application's own `auth::settings()` in `src/auth.rs` *builds* one. Call the
builder once, from `main`; read the installed one everywhere else.

They are only the same object as long as nobody adjusts the policy before
installing it — and the builder goes through `AuthSettings::from_env`, which
refuses to start a release build on a bad `AUTH_SECRET`. A request handler is
no place to call something that can exit the process.

Reading the session costs nothing and hits no database, which also means it is
only as fresh as the last `sign_in`. Load the row when the value has to be
current, and call `sign_in` again after changing anything the payload carries.

## Protecting an RPC

```rust
#[rpc(auth)]
pub async fn rename(name: String) -> rahti::Result<user::Public> { … }
```

`auth` is the only argument `#[rpc]` accepts. The check runs before the payload
is read and answers `401` with `{"error": …}` — the shape `pp.rpc` already
throws on, so the client needs no special case.

**A route guard is not a substitute.** The guard decides who may load a page;
an rpc is a POST anyone can send, and a session that expires while a page is
open still has to be refused. Mark every rpc that reads or writes something
private, whether or not its page is listed in `private_routes`.

The identity comes from the session, never from the payload:

```rust
#[rpc(auth)]
pub async fn rename(name: String) -> Result<user::Public> {
    let signed_in = auth::current().expect("`#[rpc(auth)]` answered");
    // …operate on signed_in.id. An `id` parameter would let anyone
    // rename anyone.
}
```

`#[rpc(auth)]` registers under the same name and is dispatched identically.
The generated manifest marks the name in the route's `authRpcs` list (or sets
`auth: true` on a component RPC) so development tooling can report the
requirement. The generated router still enforces it at request time.

## Redirect safety

`?next=` arrives in a URL anyone can write, so it is checked in two places and
is never echoed back as it arrived:

- `rahti::auth` normalizes it before redirecting to it.
- The sign-in page checks it again before answering with it, because the
  value it echoes is the one that reaches the browser.

A scheme, a host, a protocol-relative `//host`, or a backslash falls back to
the configured default. Never build a redirect target from an unchecked request
parameter.

### PulsePoint's half of `?next=`

`?next=` is a shared convention, not a Rahti invention: the bundled PulsePoint
v2 runtime — `public/js/pp-reactive-v2.min.js` — knows the parameter by name
and completes it on the client. Two behaviours matter here:

- A server redirect target is accepted only when it is same-origin, and is kept
  whole: path, query and fragment.
- During an SPA navigation, when the response was redirected and the target
  carries a `next` parameter, the runtime **appends the original fragment to
  `next`**.

That last part is the piece the server cannot do. A browser never sends the
fragment, so an SPA click on `/account#billing` reaches Rust as `/account`, and
the guard can only write `?next=/account`. The runtime re-attaches `#billing`,
so the URL becomes `/signin?next=/account%23billing` and the visitor lands back
on the exact place they were headed.

The two halves are complementary — the runtime never *creates* `?next=`, it only
enriches one the server produced. Removing the server half would leave that
client behaviour with nothing to act on.

Verified round trip, signed out:

```text
SPA click /account#billing
  -> 303 Location: /signin?next=/account        (rahti::auth::guard)
  -> /signin?next=%2Faccount%23billing          (the runtime adds the fragment)
  -> sign in
  -> /account#billing
```

This is why the sign-in page's own `next` check must accept a `#` in the
value: by the time it comes back, the client has put one there.

## Passwords

Application-owned, in `src/auth.rs`. What a sound implementation looks like:

- argon2id with a fresh random salt per user, stored as a PHC string.
- A length floor and nothing else. Composition rules make passwords harder to
  remember and barely harder to guess.
- One error message for "no such user" and "wrong password", **and** a hash
  verified against a constant even when there is no user. Both are the same
  defence: a sign-in that fails faster or differently for an unknown address
  enumerates your users for whoever asks.
- Emails normalized before they are stored or looked up, with a unique index in
  the migration — the application's duplicate check is for the readable error;
  the constraint is what makes it true under a race.
- A wire type (`user::Public`) with no field for the hash, so what may leave the
  server is answered by a type rather than by care.

## Testing

The application test harness installs the same policy `main` does:

```rust
rahti::auth::configure(crate::auth::settings());
```

Without it a suite exercises private routes that nothing protects. A suite
that signs in and then calls a protected rpc needs to carry both cookies —
the session and the CSRF token — from one response to the next request, the
way a browser would.

## What is not here

- **OAuth providers.** No Google, no GitHub, no provider abstraction.
- **Roles and permissions.** A call is from a session or it is not. Anything
  finer is a question about your users; answer it in your own code, in the rpc
  body, where the row is in hand.
- **Server-side revocation.** The session is stateless. Sign-out clears the
  cookie; it does not invalidate one already copied elsewhere.
- **Remember-me, refresh tokens, device lists, MFA.**
