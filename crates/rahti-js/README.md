# rahti-js

Compiles the client code in [Rahti](https://github.com/TheSteelNinjaCode/rahti)
templates ahead of time. Not used directly: `html!` calls it at compile time
for every `{binding}`, inline handler and component `<script>`, and registers
the result in the application's client registry, which the browser runtime
loads from `/_rahti/client.js`.

Because the browser never turns page text into code, a Rahti page needs no
`'unsafe-eval'` in its Content-Security-Policy, and markup injected into a
page stays text.

Each input is parsed with a JavaScript parser, so a binding that is not
JavaScript is a compile error on the template rather than a broken page.

It also lists the `pp.rpc` calls a piece of client code makes — the name and
the payload's keys — so `html!` can check each one against its `#[rpc]` and
refuse a misspelled name or key at compile time.
