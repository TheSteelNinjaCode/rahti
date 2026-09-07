# cargo-rahti

Create and maintain [Rahti](https://crates.io/crates/rahti) projects.

```bash
cargo install cargo-rahti
```

## `cargo rahti new`

```bash
cargo rahti new my-app [--tailwind] [--db [sqlite|postgres|mysql]] [--ws]
```

Every flag adds a feature, and leaving it out is how you say no; an
interactive run asks about whatever was not named. A bare `--db` is SQLite.

A new project compiles and serves on the first `cargo run`. It carries:

- a working page with a server RPC, a root layout, and a stylesheet for the
  engine you chose (Tailwind or plain CSS);
- the PulsePoint browser runtime as a prebuilt asset — there is no
  JavaScript build step;
- a generated, gitignored `.env` with a per-project session signing secret and
  cookie name; what a clone has to supply is documented at
  <https://rahti.tsnc.tech/docs/environment>, and a release build
  refuses to start on a placeholder secret;
- with `--db`: SeaORM dependencies, a worked example entity and migration,
  and the process-wide connection in `src/db.rs`;
- with `--ws`: the `ws` feature on the `rahti` dependency, which compiles
  `#[socket]` and `rahti::ws`;
- its own documentation: `AGENTS.md` — a project guide written for coding
  agents and the people beside them — and the framework's convention
  documents under `docs/conventions/`.

## `cargo rahti upgrade`

```bash
cargo rahti upgrade [--dry-run] [--db [backend]] [--ws]
```

Every file the scaffold writes is recorded in `rahti.config.json` with its
hash at the moment of writing. `upgrade` compares: a file whose hash still
matches is the scaffold's and is brought up to date; one you edited is yours
and is left alone with a note; one you deleted stays deleted. `Cargo.toml` is
never regenerated from the template — your dependencies and a `--local` path
dependency are yours.

`--db` and `--ws` add a feature to a project that started without it, and
finish the job: the files are written, the choice is recorded, and the
manifest and `.env` are amended with the dependency, feature and connection
string the new files need. Additions only, and never twice — running it again
changes nothing.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
