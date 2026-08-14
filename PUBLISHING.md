# Publishing

The crates depend on each other, so they go to crates.io in dependency order.
Each `cargo publish` is **irreversible** — a version can be yanked but never
removed, and the name is taken permanently.

## Before the first release

- [x] **Add `LICENSE-APACHE`.** Copied verbatim from
      <https://www.apache.org/licenses/LICENSE-2.0.txt>.
- [x] **READMEs.** The workspace `README.md` is the framework's face and the
      `rahti` crate's page (`readme = "../../README.md"`); every other
      publishable crate carries its own.
- [ ] **Set the real repository URL** in `[workspace.package]` of the root
      `Cargo.toml`. The current value is a placeholder.
- [ ] **Decide whether `rahti-ui` should ship at all.** It is the example
      component library this repository tests against — a fuchsia button and a
      counter. Publishing it claims the name; not publishing it leaves the
      name free for the real thing. Set `publish = false` if it is only a
      fixture.
- [ ] `cargo test --workspace` clean.
- [ ] Commit everything. `cargo publish` records the git revision.

## Order

Each crate can only be published once everything it depends on is already on
crates.io, so the order is forced:

```bash
cargo publish -p rahti-macros
```

```bash
cargo publish -p rahti-build
```

```bash
cargo publish -p rahti
```

```bash
cargo publish -p cargo-rahti
```

`rahti-ui` last, if it is being published at all.

Wait for each to appear on the index before the next — crates.io takes a
moment, and the following publish resolves against it.

## Why `cargo package -p rahti` fails today

```
no matching package named `rahti-macros` found
```

That is not a defect. `cargo package` verifies by building the packaged crate
against crates.io, and `rahti-macros` is not there yet. It resolves the moment
the first publish lands. `rahti-macros` and `rahti-build` both package and
verify cleanly today, which is as far as this can be checked before release.

## The version lockstep

`[workspace.package] version` is shared, and `[workspace.dependencies]` pins
each intra-framework dependency to that same number. This is deliberate:
`html!` expands to paths in `rahti`, and `rahti-build` writes code calling
functions in it, so a project that mixed versions would fail to compile with
an error naming neither crate. Release them together, always.

## After publishing

Check that the scaffold actually works from the registry rather than from this
checkout — that is the whole promise, and `--local` has been hiding it:

```bash
cargo install cargo-rahti
```

```bash
cargo rahti new smoke-test --tailwind
```

The generated `Cargo.toml` should carry `rahti = "0.0.11"`, not a path.
