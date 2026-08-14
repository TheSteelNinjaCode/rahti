# Rahti Copilot Instructions

Rahti is a server-rendered Rust framework whose browser reactivity comes from
the bundled PulsePoint v2 runtime. This repository is the framework workspace
— the crates under `crates/` and the canonical documentation under
`docs/conventions/`; applications are scaffolded separately with
`cargo rahti new`. Read `AGENTS.md` before making changes and follow the
detailed documents in `docs/conventions/`.

Essential rules:

- In an application, `rahti-build` owns the generated files (`src/routes.rs`,
  `src/components/mod.rs`, `.rahti/manifest.json`); they are never
  hand-edited, so generation must stay deterministic and complete.
- Routes are filesystem conventions under an application's `src/app/`.
- In `html!`, `@{...}` is a Rust/server expression and `{...}` is a
  PulsePoint/JavaScript browser expression.
- Components are PascalCase `#[component]` functions returning one-root
  `Html`. Children are an explicit `Html` argument placed with `<slot />`.
- Keep a reactive block's markup and `<script>` state self-contained.
- Page RPCs live in their `page.rs`; component RPCs in the component file.
  Use owned deserializable parameters and serializable return values.
- The browser runtime is a prebuilt asset whose canonical copy is
  `crates/cargo-rahti/assets/js/pp-reactive-v2.min.js`. There is no
  JavaScript build step; never hand-edit the bundle.
- Convention documents (except `architecture.md` and `testing.md`) ship into
  new projects from `crates/cargo-rahti/assets/docs/`; a test holds each copy
  byte-identical to its canonical file under `docs/conventions/`. Edit both
  together.

Canonical details:

- `docs/conventions/routing.md`
- `docs/conventions/rendering-and-components.md`
- `docs/conventions/pulsepoint.md`
- `docs/conventions/diagnostics.md`
- `docs/conventions/rpc-and-uploads.md`
- `docs/conventions/security.md`
- `docs/conventions/configuration-and-tooling.md`
- `docs/conventions/testing.md`
