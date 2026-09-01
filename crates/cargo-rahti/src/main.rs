//! `cargo rahti` — create and maintain Rahti projects.
//!
//! Cargo turns `cargo rahti <args>` into `cargo-rahti rahti <args>`, so the
//! first argument is the subcommand's own name and is skipped. Running the
//! binary directly works too, which is what the tests do.
//!
//! Arguments are read by hand rather than with a parser crate. The surface is
//! a verb, a name and three flags — each of which adds something, so there
//! are no negations to reconcile; a dependency that reads them would be
//! larger than the code that acts on them.

mod native;
mod new;
mod prompt;
mod studio;
mod templates;
mod upgrade;
mod wiring;

use std::process::ExitCode;

/// Written into `createdWith`, so a project records the version that made it.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `cargo rahti new app` arrives as `["rahti", "new", "app"]`; running the
    // binary directly gives `["new", "app"]`. Both are supported.
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .skip_while(|a| *a == "rahti")
        .collect();

    let result = match args.first().copied() {
        Some("new") => new::run(&args[1..]),
        Some("upgrade") => upgrade::run(&args[1..]),
        Some("studio") => studio::run(&args[1..]),
        // Handed to `cargo-rahti-native`, which is a separate install. Returns
        // rather than falling through to the `Result` below, because what the
        // helper exited with is what this should exit with.
        Some("native") => return native::run(&args[1..]),
        _ => return other(&args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Everything that is not a command that does work.
fn other(args: &[&str]) -> ExitCode {
    match args.first().copied() {
        Some("--version" | "-V") => {
            println!("cargo-rahti {VERSION}");
            ExitCode::SUCCESS
        }
        Some("help" | "--help" | "-h") | None => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: `{other}` is not a cargo-rahti command.\n");
            eprint!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    format!(
        "cargo-rahti {VERSION}
Create and maintain Rahti projects.

USAGE:
    cargo rahti new <name> [options]
    cargo rahti upgrade [--dry-run] [--force [path...]] [--db [backend]] [--ws] [--mcp]
    cargo rahti studio [--port <port>]
    cargo rahti native <command> [options]

NEW:
    A flag adds a feature. Leaving it out is how you say no.

    --tailwind         Add Tailwind CSS. Without it, plain CSS.
    --db [backend]     Add a database — sqlite, postgres or mysql. Bare
                       `--db` is sqlite, the one that needs no server.
                       Writes src/models/ and src/migrations/, and adds
                       SeaORM to Cargo.toml.
    --ws               Add WebSockets: the `ws` feature on the rahti
                       dependency, which compiles `rahti::ws` and the
                       `#[socket]` attribute.
    --mcp              Add Rahti's MCP developer tooling configuration.
                       Requires `cargo install rahti-mcp` once per machine.
    --local <path>     Depend on a Rahti checkout by path rather than by
                       version. For working on the framework itself.

UPGRADE:
    -n, --dry-run      Print what would change, and change nothing.
    -f, --force [path...]
                       Take back scaffolded files you have edited. Bare,
                       it takes the framework's own — src/main.rs, build.rs,
                       the docs, the PulsePoint assets — and leaves
                       everything else under src/, which is your
                       application. Name paths to take those too, including
                       an application file. Pair it with --dry-run first:
                       the replaced version is not kept anywhere.
    --db [backend]     Add a database to a project that has none — the same
                       flag `new` takes. Writes the files, records the
                       backend in rahti.config.json, and adds SeaORM to
                       Cargo.toml and DATABASE_URL to .env.
    --ws               Add WebSockets to a project without them, feature and
                       all.
    --mcp              Add MCP tooling to a project without it, including
                       the recorded .mcp.json client configuration.

    Rewrites the scaffolded files you have not edited, and leaves the ones
    you have. Which is which is decided by the hashes in rahti.config.json,
    not by guessing. A dry run never asks a question — name the feature to
    preview adding it.

    Cargo.toml and .env are never regenerated, because your dependencies and
    your credentials are yours — but they are amended when a feature needs
    it, by additions only, and never the same one twice.

STUDIO:
    Starts Prisma Studio for this Rahti project's configured database. It
    reads DATABASE_URL from the environment or .env, without printing it.
    Prisma is an optional developer dependency: install it yourself first
    with `npm install --save-dev prisma`, then run this command.

    --port <port>     Port for the local Prisma Studio server.

NATIVE:
    Optional Windows and Android packaging, in a separate tool so that a web
    project never resolves a native dependency:

        cargo install cargo-rahti-native

    Once it is installed, `cargo rahti native init`, `doctor`, `dev` and
    `build` are forwarded to it. Run `cargo rahti native --help` for those.

    -h, --help         Print this message.
    -V, --version      Print the version.

An interactive run asks about any feature you did not name. A run with
nowhere to ask — a pipe, a CI job — takes the flags at their word and adds
only what they list.
"
    )
}
