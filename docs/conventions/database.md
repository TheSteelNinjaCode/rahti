# Database

Rahti does not have a database layer. It has a place to put one, and wiring
that keeps it from drifting.

The database is [SeaORM 2](https://www.sea-ql.org/SeaORM/), and it is the
*application's* dependency. `rahti` does not depend on it, `rahti-macros`
does not know it exists, and `rahti-build` generates the module files without
reading a line of SeaORM code. Nothing in this document can be assumed to
happen magically: if it is not listed under "What the build generates", the
application wrote it.

## Turning It On

At scaffold time:

```text
cargo rahti new my-app --db sqlite      # or postgres, or mysql
cargo rahti new my-app --db             # bare --db is sqlite
cargo rahti new my-app                  # no database
```

The flag adds a database, so leaving it out is how you say no. Without it an
interactive run asks — defaulting to no, so pressing enter agrees with
omitting the flag — and a run with nowhere to ask takes the command line at
its word.

For an existing project, one command:

```text
cargo rahti upgrade --db sqlite
```

It writes `src/db.rs`, the first entity and its migration, records the backend
in `rahti.config.json`, and then finishes the job in the two files it never
regenerates: `sea-orm` and `sea-orm-migration` are appended to
`[dependencies]`, pinned to major version 2 with the feature for your backend
(`sqlx-sqlite`, `sqlx-postgres`, `sqlx-mysql`), and a `DATABASE_URL` is added
to `.env`.

Those two are additions, not rewrites. Everything else in the manifest stays
as it is — your dependencies, your versions, and a `--local` project's path
dependency — and everything else in `.env` stays where it was, with the
connection string above it. Nothing is added twice, so running the command
again changes nothing.

For a backend other than SQLite the connection string it writes is an
example, and pointing it at your server is the one step left. If the manifest
is in a shape the addition cannot read — a `[dependencies.rahti]` table, say —
it says so and prints the line for you to add.

`backend` decides one thing: which cargo feature the manifest needs. A
misspelling is refused by the build rather than defaulted, for the same reason
`css.engine` is — falling back to SQLite would wire a database nobody is
talking to, and falling back to none would leave `crate::models` undeclared
with nothing to explain it.

There is no connection string in `rahti.config.json`. That file is committed
and a connection string is a credential. The URL is read from `DATABASE_URL`.

## Layout

```text
src/
├── db.rs                                  the connection — yours
├── models/
│   ├── mod.rs                             generated — do not edit
│   └── todo.rs                            one file per table — yours
└── migrations/
    ├── mod.rs                             generated — do not edit
    └── m20260101_000001_create_todo.rs    one file per change — yours
.env                                       DATABASE_URL — gitignored
```

Both directories are **flat**. A table name is a flat namespace — there is no
`users::billing` table — and a subdirectory is refused by the build rather
than quietly given a module path the database has no counterpart for.

## What the Build Generates

`rahti-build` writes two files, from the contents of the two directories, on
every build. Neither is edited by hand.

**`src/models/mod.rs`** declares every `.rs` file beside it. Saving
`src/models/invoice.rs` is the whole of making `crate::models::invoice`
exist.

**`src/migrations/mod.rs`** declares every `.rs` file beside it *and* writes
the `Migrator`:

```rust
pub struct Migrator;

impl ::sea_orm_migration::MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn ::sea_orm_migration::MigrationTrait>> {
        vec![
            Box::new(m20260101_000001_create_todo::Migration),
        ]
    }
}
```

This is the one that matters. SeaORM's migrator is normally a `Vec` you
maintain by hand, and a migration written but never pushed into it is the
quietest failure available: the file is there, the table is not, and nothing
anywhere says why. Generated from the directory, that state cannot be reached.

Order is filename order, and filename order is application order. **Name a
migration for the moment it was written** — `m20260101_000001_create_todo.rs`
— so the two agree.

Both directories are created if missing, because `main.rs` declares the
modules unconditionally once a project has a database.

## Reaching the Connection

```rust
use crate::db::db;

let rows = todo::Entity::find().all(db()).await?;
```

`db()` is a process-wide `OnceLock<DatabaseConnection>`, installed by
`db::connect_if_configured()` before the listener binds when `DATABASE_URL`
is configured. A deployment can start without the variable, which lets its
owner choose and add their connection string later; database-using pages and
RPCs must check `db::is_connected()` and return a useful unavailable state
instead of calling `db()` while no connection exists.

It is a global rather than a handler argument because **Rahti handlers take no
state**. A `page()` is a plain function, an `#[rpc]` is a plain function, and
the generated router calls `Router::new()` with no state type — so there is
nowhere for a pool to be threaded through. This is not a workaround:
`DatabaseConnection` is an internally-pooled handle that is cheap to clone and
meant to be shared.

`src/db.rs` is yours. Pool size, statement timeouts, a read replica — they go
there.

## Adding a Table

Four steps, always the same. Copy `src/models/todo.rs` and
`src/migrations/m20260101_000001_create_todo.rs` rather than writing from
memory; they are commented as the worked example.

**1. The migration**, in `src/migrations/mYYYYMMDD_NNNNNN_<what>.rs`:

```rust
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Invoice::Table)
                    .if_not_exists()
                    .col(pk_auto(Invoice::Id))
                    .col(string(Invoice::Reference))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Invoice::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Invoice {
    Table,
    Id,
    Reference,
}
```

Write `down`. A `down` you never run costs a minute; the one time you need it
you need it badly.

The `DeriveIden` enum stays beside the migration rather than being shared with
the entity. It describes the table *as it was at this version*, and a later
migration that renames a column must not change what an earlier one did.

**2. The entity**, in `src/models/<table>.rs`, named in snake_case after the
table:

```rust
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "invoice")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub reference: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
```

`Serialize` and `Deserialize` are not optional in practice: they are what let
an `#[rpc]` return `Model` directly, so there is no second struct to keep in
step with the table.

**The entity does not create the table.** Nothing is checked against the
database at startup and nothing is read at build time — an entity that
disagrees with the schema compiles cleanly and fails on the first query. The
migration is the schema; the entity is how Rust talks to it.

**3. Nothing.** Both `mod.rs` files regenerate on the next build.

**4. A test.** See below.

## Migrations Are Not Automatic

`db::migrate()` exists and is never called by `db::connect_if_configured()`.

A framework that changes your schema because you started the server is the
same class of surprise as one that downloads a compiler you did not ask for,
and the production version of that surprise is much worse. Call it
deliberately — from a `main` branch behind an argument, or from a test.

The one defensible exception is a demo whose database is a throwaway SQLite
file a fresh clone does not have — where the point is that `cargo run` works
on the first try — and its `main` should say so in a comment.

## Errors

`DbErr` needs no conversion:

```rust
#[rpc]
pub async fn list() -> Result<Vec<todo::Model>> {
    Ok(todo::Entity::find().all(db()).await?)
}
```

`rahti::Error` implements `From<E>` for every `E: std::error::Error`, so a
database failure becomes a 500 carrying its message the moment it is
returned. Nothing was written to make this work.

Use a real status where you have one. A missing row is a 404, not a 500:

```rust
let existing = todo::Entity::find_by_id(id)
    .one(db())
    .await?
    .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "no todo with that id"))?;
```

A `page()` returning `Html` has no way to report a failure. Either return
`rahti::Result` so the nearest `error.rs` catches it, or decide what an
unreachable database renders — an empty list is often the honest answer.

## Validation Is Still Yours

A database that would accept an empty string is not a reason to store one.
Use `rahti::Validator` in the `#[rpc]` before the query. Invalid typed input
returns a structured 422 response; see `validation.md`. Keep database
constraints for invariants such as uniqueness because two valid requests can
still race after an application-level check.

## Testing

SQLite needs no server, so a whole suite runs against a real engine. Connect
once for the test binary:

```rust
async fn database() {
    static READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

    READY
        .get_or_init(|| async {
            let path = std::env::temp_dir().join("my-app-tests.db");
            let _ = std::fs::remove_file(&path);

            let url = format!("sqlite://{}?mode=rwc", path.display());
            crate::db::connect_to(&url).await.expect("a test database");
            crate::db::migrate().await;
        })
        .await;
}
```

**Use a file, not `:memory:`.** This is the trap, and it costs an hour if you
walk into it. The connection is a process-wide static, but every
`#[tokio::test]` builds its own runtime and drops it when the test ends — so
the pool that opened the database belongs to a runtime that is gone by the
time the next test runs. An in-memory database exists only while something is
connected to it, so it disappears with that runtime and every later test fails
with `no such table` on a table the migration certainly created. Worse, the
tests pass one at a time and fail together, which reads like a bug in the code
under test.

Delete the file on the way in, not on the way out: a test binary has no
reliable teardown, and starting from nothing is what makes a run repeatable.

Call the setup from the request helpers rather than from each test — pages
that touch the database are reached by tests that are not about the database
at all.

## Security

- `DATABASE_URL` lives in `.env`, which is gitignored. Nothing is committed in
  its place — the environment reference at
  <https://rahti.tsnc.tech/docs/environment> says what a clone has to supply,
  so there is no second copy of it to go stale.
- A real environment variable always beats the file. `src/db.rs` reads `.env`
  without overriding what is already set, so a deployment that exports
  `DATABASE_URL` cannot be silently redirected by a stale file on the same
  machine.
- SeaORM parameterises queries. If you drop to raw SQL, bind values — do not
  format them into the string.
- What a page renders is escaped by `html!` regardless of where the value came
  from. A row read from the database is not trusted markup; `Html::from_raw`
  is still the only way to say otherwise, and a database is a poor reason to
  say it.

## The Manifest

`.rahti/manifest.json` reports what is wired in:

```json
"db": {
  "backend": "sqlite",
  "models": ["src/models/todo.rs"],
  "migrations": ["src/migrations/m20260101_000001_create_todo.rs"]
}
```

`null` when the project has no database. A directory listing answers "what
files are there"; this answers "what did the build actually wire up", which is
the question that has been wrong before.

## Failure Modes

| Symptom | Cause |
| --- | --- |
| `db.backend is "…", which is not a backend` | Misspelled in `rahti.config.json`. Not defaulted, deliberately. |
| `the database has not been connected` | `db()` was reached while `DATABASE_URL` is absent, or before `db::connect_if_configured()`. Guard the page or RPC with `db::is_connected()`. |
| `DATABASE_URL` is absent | This is allowed. The app starts without persistence until its owner configures a connection string and restarts it. |
| `no such table` after a migration was written | The migration ran but the table is not there: almost always an in-memory SQLite that outlived its runtime. See Testing. |
| `unresolved import sea_orm` | The config has a `db` object and `Cargo.toml` does not have the dependency. Run `cargo rahti upgrade`, which adds it. |
| `… is a directory, and src/models is flat` | An entity in a subdirectory. Move it up. |
| A migration file exists but never runs | It cannot — the `Migrator` is generated from the directory. Check the file is `.rs`, is not `mod.rs`, and does not start with `.`. |
| An entity compiles but every query fails | The entity and the schema disagree. Nothing checks them against each other; read the migration. |
