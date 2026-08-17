//! `cargo rahti native …` — handing over to the optional packaging tool.
//!
//! Native packaging lives in a separate executable, `cargo-rahti-native`, and
//! that separation is the whole of what makes it optional: `cargo-rahti` is
//! what every Rahti project uses, and nothing here depends on Tauri, on an
//! Android SDK, or on `rahti-native`. This file is a `Command` spawn.
//!
//! ## Why the delegation exists at all
//!
//! Cargo already dispatches `cargo rahti-native …` to the installed binary on
//! its own — a `cargo-<name>` executable on `PATH` becomes `cargo <name>`, no
//! cooperation needed. So this adds nothing cargo cannot do.
//!
//! What it adds is the answer to a person who types the obvious thing.
//! `cargo rahti new` and `cargo rahti upgrade` exist, so `cargo rahti native`
//! is what somebody tries, and without this they get "`native` is not a
//! cargo-rahti command" — which is true and tells them nothing. With it they
//! get the tool, or the one line that installs it.
//!
//! ## What it will not do
//!
//! Install anything. A tool that reacted to a missing dependency by fetching
//! and running one is a tool that decides what runs on somebody's machine.
//! The message names the command; the person runs it.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[cfg(test)]
#[path = "tests/native.rs"]
mod tests;

/// The executable this forwards to.
pub const HELPER: &str = "cargo-rahti-native";

/// Run `cargo-rahti-native native <args>`, and exit with whatever it exits
/// with.
///
/// The leading `native` is kept rather than stripped: the helper accepts it
/// among the words cargo's own dispatch would have put there, so one argument
/// vector serves both routes in.
pub fn run(args: &[&str]) -> ExitCode {
    let mut command = Command::new(locate());
    command.arg("native").args(args);

    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        // The helper has already said whatever it had to say. Repeating it, or
        // wrapping it in a `cargo-rahti` error, would only make the real
        // message harder to find.
        Ok(_) => ExitCode::FAILURE,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprint!("{}", missing());
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: could not run `{HELPER}`: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What to print when the helper is not installed.
///
/// Separated from the spawning so the wording can be tested, because the
/// wording is the entire value of this file.
pub fn missing() -> String {
    format!(
        "Native tooling is optional.
Install it with:
    cargo install {HELPER}

It packages an existing Rahti application for Windows and Android: the same
pages, the same rpcs and the same sockets, served to a system WebView by the
application's own Rust backend running inside the installed program.

A web project needs none of it, which is why `cargo-rahti` does not carry it.
"
    )
}

/// Where to find the helper.
///
/// Beside this executable first, then `PATH`.
///
/// The sibling case is not a convenience: `cargo install` puts both binaries
/// in the same directory, and a workspace checkout puts both in the same
/// `target/debug`. Looking there first means a person testing a local build of
/// one gets the local build of the other, rather than whichever version they
/// happen to have installed globally.
pub fn locate() -> PathBuf {
    let name = format!("{HELPER}{}", std::env::consts::EXE_SUFFIX);

    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|path| path.is_file());

    // Bare, so the operating system searches `PATH` — which is where
    // `cargo install` put it for everybody who did not build it themselves.
    sibling.unwrap_or_else(|| PathBuf::from(name))
}
