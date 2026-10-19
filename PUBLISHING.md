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
cargo publish -p rahti-mcp
```

```bash
cargo publish -p rahti
```

```bash
cargo publish -p cargo-rahti
```

Wait for each to appear on the index before the next — crates.io takes a
moment, and the following publish resolves against it.

## Why `cargo package -p rahti` can fail before its turn

```
no matching package named `rahti-mcp` found
```

That is not a defect. `cargo package` verifies by building the packaged crate
against crates.io, and its internal dependencies are not there yet. It
resolves once `rahti-macros` and `rahti-mcp` for the release have landed.
Publish strictly in the dependency order above.

## The version lockstep

`[workspace.package] version` is shared, and `[workspace.dependencies]` pins
each intra-framework dependency to that same number. This is deliberate:
`html!` expands to paths in `rahti`, and `rahti-build` writes code calling
functions in it, so a project that mixed versions would fail to compile with
an error naming neither crate. The optional HTTP feature makes `rahti-mcp` a
dependency of `rahti`, so it must publish first. Release them together,
always.

## After publishing

Check that the scaffold actually works from the registry rather than from this
checkout — that is the whole promise, and `--local` has been hiding it:

```bash
cargo install cargo-rahti
```

```bash
cargo rahti new smoke-test --tailwind
```

The generated `Cargo.toml` should carry the version just published — the
scaffold writes its own `CARGO_PKG_VERSION` — and not a path.
