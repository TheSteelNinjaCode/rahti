# Security Middleware

Rahti's generated router wraps the complete application in one browser
security-header middleware. It covers page responses, RPC success and error
responses, API routes, redirects, production probes, development endpoints,
404s, and files from `public/`. Applications do not add the layer themselves.
It is the outermost browser-policy layer and sits immediately inside production
observability, which adds correlation metadata after the downstream response.

The middleware adds a header only when the response has not already set it.
A handler, nested router, or hosting layer may therefore apply a stricter
per-response value without the framework overwriting it.

## Default headers

| Header | Default | Purpose |
| --- | --- | --- |
| `Content-Security-Policy` | PulsePoint-compatible baseline | Restricts resource loading, form targets, framing, base URLs, and plugins |
| `Permissions-Policy` | camera, geolocation, microphone, payment, and USB limited to this origin | Prevents embedded third-party frames from receiving those capabilities |
| `Referrer-Policy` | `strict-origin-when-cross-origin` | Keeps paths and queries out of cross-origin referrers |
| `X-Content-Type-Options` | `nosniff` | Prevents browser MIME guessing |
| `X-Frame-Options` | `SAMEORIGIN` | Clickjacking protection for browsers that do not read CSP `frame-ancestors` |
| `Strict-Transport-Security` | one year, this host only | Enforces HTTPS outside development mode only |

HSTS is intentionally absent when Rahti development mode is active. In a
deployment it is sent even when TLS terminates at a reverse proxy: the browser
consumes the header from the final response. The default,
`max-age=31536000`, covers this host only. Add `includeSubDomains` only when
every subdomain of the site is served over HTTPS: a browser that has seen it
refuses plain HTTP on all of them for the whole `max-age`, and no later
response can take that back. Set the field to `None` when the proxy owns HSTS.
An application whose every subdomain is HTTPS opts in during startup:

```rust
rahti::security::configure(SecurityHeaders {
    strict_transport_security: Some("max-age=31536000; includeSubDomains".to_string()),
    ..SecurityHeaders::default()
});
```

## Content Security Policy

The default CSP is a compatibility baseline, not an application-specific host
allowlist. Its important locked directives are:

```text
default-src 'self'
script-src 'self'
object-src 'none'
base-uri 'self'
form-action 'self'
frame-ancestors 'self'
require-trusted-types-for 'script'
trusted-types pulsepoint
```

`script-src` is `'self'` and nothing else: no `'unsafe-eval'` and no
`'unsafe-inline'`. PulsePoint never turns page text into code. Every binding,
handler and component script is compiled when the application is built and
served from `/_rahti/client.js`, which `public/js/main.js` imports before it
mounts the page; the runtime runs only what that registry holds. Authored
`onclick={...}` handlers travel in inert compiler-owned attributes and are
bound through event listeners. The browser parser never treats them as inline
JavaScript, even while initial HTML or detached render trees are parsed.
So an injected
inline `<script>`, an `onerror=` attribute, a `<script src>` on another host,
and an injected `{…}` binding all stay inert — the browser refuses the first
three and reports a violation, which `rahti::browser` surfaces in `errors()`,
and the runtime renders the last as text.

`require-trusted-types-for 'script'` makes the browser refuse a plain string at
every sink that parses markup or code (`innerHTML`, `DOMParser`, a script's
text, an `on…` attribute). `trusted-types pulsepoint` lets exactly one policy
exist — PulsePoint's — so the runtime is the only code on the page that can
write markup. A library that assigns `innerHTML` with a string throws; give it
an element to own through `pp.ref` and its own DOM API, or replace it.

An application that loads a vendor SDK names that host in `script-src`
(`script-src 'self' https://js.stripe.com`); never `https:`, `'unsafe-inline'`
or `'unsafe-eval'`, any of which lets injected markup run script. An SDK that
creates its own Trusted Types policy needs its policy name added to
`trusted-types`. An application that compiles WebAssembly adds
`'wasm-unsafe-eval'`, which allows WebAssembly and nothing else. The default
permits HTTPS styles, fonts, images, media, frames, and connections, which
cannot run script in the page. Images and media additionally permit `data:` and
`blob:` where appropriate, and connections permit `ws:`/`wss:`. Applications
that know their external dependencies can replace these scheme allowances with
exact hosts. Keep the locked directives above. In development, loopback HTTP
connection sources and cleartext media/frames are admitted for local tooling;
production omits them.

## Application configuration

Defaults require no startup call. To replace or disable individual headers,
install a complete policy during `initialize_application`, before building the
router:

```rust
use rahti::security::SecurityHeaders;

rahti::security::configure(SecurityHeaders {
    content_security_policy: Some(
        "default-src 'self'; \
         script-src 'self' https://js.stripe.com; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         connect-src 'self' ws: wss:; \
         object-src 'none'; base-uri 'self'; \
         form-action 'self'; frame-ancestors 'self'; \
         require-trusted-types-for 'script'; trusted-types pulsepoint"
            .to_string(),
    ),
    // This deployment's proxy owns HSTS.
    strict_transport_security: None,
    ..SecurityHeaders::default()
});
```

Every field is an `Option<String>`; `None` omits that header. A malformed
header value is rejected during `configure` instead of failing silently while
serving requests. A later `configure` replaces the complete installed policy,
which allows router integration tests to install the same settings as
application startup.

Do not use configuration to remove a protection merely to silence a browser
console error. Read the violated directive, add the narrow source the
application actually needs, and keep the unrelated directives intact.

## Boundaries this does not replace

Security headers reduce what a browser will do with a response. They do not
validate input, authenticate a caller, authorize a resource, or prove request
origin. Keep using:

- `rahti::Validator` for application input rules;
- private routes and `#[rpc(auth)]` for authentication;
- `rahti::authorization` for abilities and resource policies;
- the generated CSRF layer for state-changing RPCs;
- escaped `html!` interpolation instead of `Html::from_raw` for untrusted data.

Tests for an application-specific CSP should request a real page through the
generated router and assert the final response header. Testing only the policy
builder does not prove that a proxy or a route-specific response left it in
place.
