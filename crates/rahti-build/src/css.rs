//! Compile the app's stylesheet, the way `scanner` compiles its routes.
//!
//! Tailwind ships a standalone CLI: one self-contained executable with the
//! engine inside, no Node and no npm. This module finds that executable and
//! runs it over `src/app/globals.css`, landing the result in `public/` where
//! the static handler already serves it.
//!
//! What it deliberately will not do is make a build depend on finding the
//! CLI. The compiled sheet is checked in — the same call `src/routes.rs`
//! makes — so a fresh clone with no CLI, or a machine with no network, still
//! serves working CSS. Compiling is an optimization over a committed file,
//! never a precondition for building.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

#[cfg(test)]
#[path = "tests/css.rs"]
mod tests;

use crate::{Css, Engine, sha256, write_if_changed};

/// Where a downloaded CLI is cached. Under `.rahti/`, which is gitignored,
/// so the binary never reaches a commit.
const BIN_DIR: &str = ".rahti/bin";

/// Where Tailwind writes before the result is published.
///
/// Pointing it straight at `public/css/styles.css` would rewrite that file on
/// every build — and the dev server watches `public/` and pushes a reload on
/// any change it sees. Compiling into `.rahti/` and passing the result
/// through [`write_if_changed`] means an unchanged sheet stays untouched and
/// the browser stays quiet.
const TMP_DIR: &str = ".rahti/css";

/// The scratch file for *this* build, stamped with the process id.
///
/// One shared path would be a race: `cargo watch` starts a rebuild while
/// another is still running, and Tailwind truncates its output before
/// filling it. The build that reads at that moment gets an empty string and
/// publishes it over a working stylesheet.
fn tmp_output() -> PathBuf {
    Path::new(TMP_DIR).join(format!("styles-{}.css", std::process::id()))
}

/// Where the standalone CLI is published.
const RELEASES: &str = "https://github.com/tailwindlabs/tailwindcss/releases/download";

/// Turn `css.entry` into `css.output`, or explain why it did not.
pub fn build(css: &Css) {
    if css.engine == Engine::None {
        return;
    }

    // No entry, nothing to build. An app that never wrote one just serves
    // whatever is in `public/` — this is not an error.
    if !Path::new(&css.entry).is_file() {
        return;
    }

    // Class names live in `.rs` files, and those are already watched by the
    // scanner, so a changed class rebuilds and re-runs the compile. This
    // line covers the remaining input: the entry sheet itself.
    println!("cargo:rerun-if-changed={}", css.entry);

    // The *output*, watched as though it were an input, so the step heals
    // itself. Without this a stylesheet that was deleted, emptied or edited
    // by hand stays broken through every later build: nothing cargo tracks
    // changed, so the build script never runs to notice. Writing it does not
    // loop, because `write_if_changed` leaves an up-to-date file untouched.
    println!("cargo:rerun-if-changed={}", css.output);

    if css.engine == Engine::Plain {
        copy(css);
        return;
    }

    let Some(binary) = resolve(css) else {
        warn(&format!(
            "no Tailwind CLI found, so {} is being served as checked in.\n  \
             Set `\"download\": true` under `css` in rahti.config.json, put \
             `tailwindcss` on PATH, or point RAHTI_TAILWIND at a binary.",
            css.output
        ));
        return;
    };

    compile(&binary, css);
}

/// The plain engine: publish `entry` as it stands.
///
/// No CLI, no download, no network — the whole point of the engine. It still
/// goes through `write_if_changed` and the same emptiness guard as a compile,
/// because the ways a stylesheet can be destroyed do not depend on what
/// produced it.
fn copy(css: &Css) {
    let out = fs::read_to_string(&css.entry)
        .unwrap_or_else(|e| panic!("rahti: cannot read {}: {e}", css.entry));

    // A Tailwind entry copied verbatim is not a stylesheet, it is a request
    // for a file called `tailwindcss` that the browser will go and fail to
    // fetch. Nothing downstream compiles these away under this engine, so
    // publishing them means a page that renders wrong with the reason four
    // files away. Refused for the same reason an empty sheet is.
    if let Some(directive) = tailwind_directive(&out) {
        panic!(
            "rahti: css.engine is \"plain\", but {} still contains `{directive}`.\n  \
             Nothing compiles it under this engine, so the browser would be \
             served it as written.\n  \
             Remove the Tailwind directives and write ordinary CSS, or set \
             css.engine back to \"tailwind\" in {}.",
            css.entry,
            crate::CONFIG_PATH
        );
    }

    let existing = fs::read_to_string(&css.output).unwrap_or_default();
    if would_erase(&out, &existing) {
        panic!(
            "rahti: {} is empty, and publishing it would replace {}, which is not.",
            css.entry, css.output
        );
    }

    write_if_changed(&css.output, &out);
}

/// Find a usable CLI, cheapest source first.
///
/// The order matters: an explicit override beats a cache, a cache beats the
/// network. Downloading is last because a build script has no business
/// reaching the network when it does not have to — and with `download =
/// false` it never does.
fn resolve(css: &Css) -> Option<PathBuf> {
    if let Some(raw) = env::var_os("RAHTI_TAILWIND") {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Some(path);
        }
        // Set but wrong: say so rather than silently falling through to a
        // different binary than the one that was asked for.
        warn(&format!(
            "RAHTI_TAILWIND is set to {}, which is not a file — ignoring it.",
            path.display()
        ));
    }

    let cached = Path::new(BIN_DIR).join(binary_name(&css.version));
    if cached.is_file() {
        return Some(cached);
    }

    if on_path() {
        return Some(PathBuf::from("tailwindcss"));
    }

    if css.download {
        return download(css, &cached);
    }

    None
}

/// Whether a `tailwindcss` on PATH answers.
///
/// Running it is the check. Guessing from PATH entries would have to
/// reimplement extension resolution on Windows and get it wrong.
fn on_path() -> bool {
    Command::new("tailwindcss")
        .arg("--help")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The cache file name. Version-stamped, so changing `version` in rahti.toml
/// fetches the new one instead of silently reusing the old.
fn binary_name(version: &str) -> String {
    format!("tailwindcss-{version}{}", env::consts::EXE_SUFFIX)
}

/// How Tailwind spells a platform in its release assets.
///
/// Also the key checksums are recorded under in rahti.toml: a hash is of one
/// specific binary, so a team on mixed machines needs one entry each rather
/// than a single value that would be wrong for everyone but its author.
///
/// A build script runs on the host, and so does the CLI it invokes, so the
/// host's own platform is the right one to ask about even when the app is
/// being cross-compiled.
pub fn platform_key(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("macos", "aarch64") => "macos-arm64",
        _ => return None,
    })
}

/// The release asset for the machine running the build.
fn asset_name(os: &str, arch: &str) -> Option<String> {
    let key = platform_key(os, arch)?;
    let suffix = if os == "windows" { ".exe" } else { "" };
    Some(format!("tailwindcss-{key}{suffix}"))
}

/// Fetch the CLI into the cache, verifying it before it is ever run.
fn download(css: &Css, dest: &Path) -> Option<PathBuf> {
    let Some(asset) = asset_name(env::consts::OS, env::consts::ARCH) else {
        warn(&format!(
            "Tailwind publishes no standalone CLI for {}-{} — install one yourself \
             and point RAHTI_TAILWIND at it.",
            env::consts::OS,
            env::consts::ARCH
        ));
        return None;
    };

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).ok();
    }

    let url = format!("{RELEASES}/v{}/{asset}", css.version);
    // A download is slow enough that a silent build looks hung.
    warn(&format!("downloading the Tailwind CLI v{}...", css.version));

    // Land it beside the target and rename on success: a half-written file
    // that keeps the real name would be treated as a cache hit next build.
    let partial = dest.with_extension("part");
    if !fetch(&url, &partial) {
        let _ = fs::remove_file(&partial);
        warn(&format!("could not download {url}"));
        return None;
    }

    let bytes = fs::read(&partial).ok()?;
    let digest = sha256::hex(&bytes);
    match &css.sha256 {
        Some(expected) if !expected.eq_ignore_ascii_case(&digest) => {
            let _ = fs::remove_file(&partial);
            panic!(
                "rahti: the Tailwind CLI downloaded from {url} does not match the \
                 checksum in rahti.toml.\n  expected {expected}\n  got      {digest}\n  \
                 Refusing to run it."
            );
        }
        Some(_) => {}
        // Unpinned. It still came over HTTPS from a pinned version tag, which
        // is the trust model `cargo install` runs on, but printing the hash
        // lets the pin be added without going and finding it by hand.
        None => warn(&format!(
            "Tailwind CLI v{} for {} has sha256 {digest}\n  \
             Pin it by adding `{} = \"{digest}\"` under [css.sha256] in rahti.toml.",
            css.version,
            platform_key(env::consts::OS, env::consts::ARCH).unwrap_or("this platform"),
            platform_key(env::consts::OS, env::consts::ARCH).unwrap_or("<platform>"),
        )),
    }

    if let Err(e) = fs::rename(&partial, dest) {
        warn(&format!("cannot move the downloaded CLI into place: {e}"));
        return None;
    }
    make_executable(dest);

    Some(dest.to_path_buf())
}

/// Retrieve `url` into `dest` with whatever the machine already has.
///
/// `curl` ships with Windows 10 and up as well as every current Unix, which
/// is what keeps this crate's dependency list at one entry instead of adding
/// an HTTP client and a TLS stack to every app's build.
fn fetch(url: &str, dest: &Path) -> bool {
    let curl = Command::new("curl")
        // Redirects are followed because the release URL is one, but only
        // ever to HTTPS.
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--tlsv1.2"])
        .arg("--output")
        .arg(dest)
        .arg(url)
        .status();

    if curl.is_ok_and(|status| status.success()) {
        return true;
    }

    // Windows installs where curl was removed still have PowerShell.
    #[cfg(windows)]
    {
        let script = format!(
            "[Net.ServicePointManager]::SecurityProtocol = 'Tls12'; \
             $ProgressPreference = 'SilentlyContinue'; \
             Invoke-WebRequest -Uri '{url}' -OutFile '{}'",
            dest.display()
        );
        let powershell = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status();
        if powershell.is_ok_and(|status| status.success()) {
            return true;
        }
    }

    false
}

/// A fetched file is not executable on Unix until it is said to be.
#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Run the CLI, then publish what it produced.
fn compile(binary: &Path, css: &Css) {
    fs::create_dir_all(TMP_DIR).ok();
    let tmp = tmp_output();

    let mut command = Command::new(binary);
    command
        .args(["--input", &css.entry])
        .arg("--output")
        .arg(&tmp);
    if minify(css) {
        command.arg("--minify");
    }

    let result = command.output().unwrap_or_else(|e| {
        panic!(
            "rahti: cannot run the Tailwind CLI at {}: {e}",
            binary.display()
        )
    });

    // A stylesheet that does not compile is a real build failure: shipping
    // the last good sheet would hide the error until someone loaded a page.
    if !result.status.success() {
        panic!(
            "rahti: Tailwind could not compile {}:\n{}",
            css.entry,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }

    let compiled = fs::read_to_string(&tmp).unwrap_or_else(|e| {
        panic!(
            "rahti: Tailwind reported success but wrote no {}: {e}",
            tmp.display()
        )
    });
    let _ = fs::remove_file(&tmp);

    let existing = fs::read_to_string(&css.output).unwrap_or_default();
    if would_erase(&compiled, &existing) {
        panic!(
            "rahti: Tailwind compiled {} to an empty stylesheet.\n  \
             Refusing to overwrite {}, which is not empty.",
            css.entry, css.output
        );
    }

    write_if_changed(&css.output, &compiled);
}

/// The first Tailwind-only directive in a sheet, if it has one.
///
/// Matched at the start of a line so that the word appearing inside a comment
/// — which is exactly where a project explaining its own migration would put
/// it — is not mistaken for a directive.
fn tailwind_directive(sheet: &str) -> Option<&'static str> {
    const DIRECTIVES: [&str; 4] = ["@import \"tailwindcss\"", "@source", "@theme", "@apply"];

    sheet.lines().find_map(|line| {
        let line = line.trim_start();
        DIRECTIVES
            .into_iter()
            .find(|directive| line.starts_with(directive))
    })
}

/// Whether publishing this result would replace a working sheet with nothing.
///
/// Nothing legitimately compiles down to an empty stylesheet while the
/// previous one had content, and the failure is close to invisible: every
/// page still renders, just unstyled, with no error anywhere to explain it.
/// Cheaper to refuse than to debug.
fn would_erase(compiled: &str, existing: &str) -> bool {
    compiled.trim().is_empty() && !existing.trim().is_empty()
}

/// Whether to minify — a setting, deliberately not a function of the profile.
///
/// Tying it to `PROFILE` looks right and is a trap: `output` is checked in, so
/// a release build would commit a minified sheet, the next dev build would
/// commit it back unminified, and the file would churn forever. One setting
/// means one output for a given input, whoever built it.
fn minify(css: &Css) -> bool {
    css.minify
}

/// Build scripts have one channel to the terminal, and this is it.
fn warn(message: &str) {
    // Cargo prints the first line prominently and indents the rest.
    println!("cargo:warning=rahti: {message}");
}
