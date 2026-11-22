# PulsePoint Navigation

SPA navigation, scroll restoration, loading regions and `pp.redirect`.
Rahti stays the URL and response authority: PulsePoint fetches the next
document from the server and replaces the page's body with it. Never add a
second client router.

## Links

- `pp.mount()` — which `public/js/main.js` calls — turns on navigation
  interception for every eligible same-origin link. There is nothing to opt
  into.
- `<a pp-spa="false">` makes that one link a normal browser navigation.
- External links, downloads, `target="_blank"` and modifier-key clicks are
  never intercepted.
- A navigation fetches the next document, replaces the whole `<body>` with
  the new one, and updates the title and the history entry. The window and
  everything a module script put on it stay. A redirect the server answers
  with is followed inside the same navigation; a response from a different
  root layout is loaded with a normal browser navigation instead.
- Every component on the page is unmounted — effect cleanups run — and the
  new document's components mount fresh. Component state does not carry
  across a navigation, including the state of a component in a layout both
  pages share. A value that must outlive a navigation lives in the URL, on
  the server, or in a store module (see "Splitting a Large Page" in
  `pulsepoint-performance.md`).
- The runtime dispatches `pp:navigation:start`, `pp:navigation:complete` and
  `pp:navigation:error` on `document`, each with the target in
  `event.detail.url`. `public/js/main.js` listens for
  `pp:navigation:complete` to carry the page's managed `<head>` metadata
  across; application code rarely needs these events.
- A navigation that fails — a response outside 2xx, a network error, or no
  response within 15 seconds — dispatches
  `pp:navigation:error`, with the failure in `event.detail.error`, and then
  loads the target with a normal browser navigation.

## `pp.redirect(url)`

After mount, `pp.redirect(url)` navigates through the SPA path for a
same-origin URL and with a normal browser navigation for any other. A
same-origin redirect an rpc answers with is followed the same way; a
cross-origin or invalid redirect target is ignored.

## Scroll

- Every history entry saves its scroll positions; Back and Forward restore
  the saved state rather than the live one.
- A push navigation resets the window to the top.
- A scrollable container that is not marked keeps its position across child
  navigations, so a shared sidebar or navigation rail stays put.
- `pp-reset-scroll="true"` on a container resets it on every navigation —
  put it on a section layout's main content pane.
- `pp-scroll-key="name"` gives a container a stable restoration identity when
  it has no stable `id` or its classes change between routes.
- `<body pp-reset-scroll="true">` resets the window and every scrollable
  element on navigation.

## Loading Regions

While a navigation is in flight the runtime replaces the first element in the
document marked `pp-loading-content="true"` — or the whole `<body>` when
nothing is marked — with the markup of the nearest loading region, then
restores it from the response. Everything outside that element stays on
screen as it is, state included, until the response replaces the body. Marking a section layout's content pane rather
than a root layout's slot keeps the chrome around it on screen for the wait.
The regions come from the application's `loading.rs` files; Rahti renders
them once and writes them into every document inside a hidden container, so
nothing is authored by hand beyond `pp-loading-content` and the optional
`pp-loading-transition` timing.

The region is chosen from the path in the address bar when the navigation
starts, which is the page being left. Ownership, the literal-path walk, the
build-time refusal of a region under a dynamic segment, and the transition
JSON are all in "Loading Regions" in `routing.md`.

An application with no `loading.rs` writes no container. A navigation then
leaves the current page on screen, unchanged, until the response arrives and
replaces the body. The same holds when no loading region covers the path
being left.

A wait inside a page — an rpc, a submit, a filter, an upload — is the owning
component's state (`pp.state`, or `pp.transition()`), never a loading region.

## Crossing Root Layouts

A body swap only makes sense while the document stays the same. When an
application has more than one root layout, the two shells have different
`<head>` contents — different stylesheets, different scripts, possibly a
different runtime setup — and swapping one body into the other would leave
the page running the wrong shell.

PulsePoint therefore compares the `X-PP-Root-Layout` header of the navigation
response with the `<meta name="pp-root-layout">` of the current document. When
both are present and they differ, it abandons the SPA path and sets
`window.location.href` to the target, which is an ordinary full load. Equal
values navigate in place as usual.

Rahti sends both halves for every page a root layout wraps, including the root
404, so nothing has to be written by hand. See "Root Layouts" in `routing.md`
for how the identity is derived and how a second root layout is declared.
