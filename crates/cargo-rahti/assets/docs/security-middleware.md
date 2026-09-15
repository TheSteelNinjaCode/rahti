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
| `X-Frame-Options` | `SAMEORIGIN` | Legacy clickjacking protection alongside CSP `frame-ancestors` |
| `Strict-Transport-Security` | one year, including subdomains | Enforces HTTPS outside development mode only |

HSTS is intentionally absent when Rahti development mode is active. In a
deployment it is sent even when TLS terminates at a reverse proxy: the browser
consumes the header from the final response. The default `includeSubDomains`
is appropriate only when every subdomain is HTTPS. Replace or disable it when
that is not true, or when the proxy owns HSTS.

## Content Security Policy

The default CSP is a compatibility baseline, not an application-specific host
allowlist. Its important locked directives are:

```text
default-src 'self'
object-src 'none'
base-uri 'self'
form-action 'self'
frame-ancestors 'self'
```

PulsePoint compiles reactive expressions in the browser, so `script-src` must
currently include both `'unsafe-eval'` and `'unsafe-inline'`. Removing either
does not harden a working Rahti application; it stops reactive blocks from
working. The default also permits HTTPS scripts, styles, fonts, images, media,
frames, and connections so ordinary SDKs and CDNs work without the framework
guessing vendor hostnames. Images and media additionally permit `data:` and
`blob:` where appropriate, and connections permit `ws:`/`wss:`.

Applications that know their external dependencies should replace this broad
scheme allowance with exact hosts. Keep the PulsePoint requirements and the
locked directives above. In development, loopback HTTP connection sources and
cleartext media/frames are admitted for local tooling; production omits them.

## Application configuration

Defaults require no startup call. To replace or disable individual headers,
install a complete policy during `initialize_application`, before building the
router:

```rust
use rahti::security::SecurityHeaders;

rahti::security::configure(SecurityHeaders {
    content_security_policy: Some(
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         connect-src 'self' ws: wss:; \
         object-src 'none'; base-uri 'self'; \
         form-action 'self'; frame-ancestors 'self'"
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
