//! `cargo rahti build --static` — run the application as its own exporter.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const OUTPUT_ENV: &str = "RAHTI_STATIC_OUTPUT";

#[derive(Debug)]
struct Options {
    output: PathBuf,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, String> {
        let mut static_mode = false;
        let mut output = PathBuf::from("dist");
        let mut at = 0;
        while at < args.len() {
            match args[at] {
                "--static" => static_mode = true,
                "--out" => {
                    at += 1;
                    let value = args
                        .get(at)
                        .ok_or_else(|| "`--out` needs a directory".to_string())?;
                    if value.trim().is_empty() {
                        return Err("`--out` needs a non-empty directory".to_string());
                    }
                    output = PathBuf::from(value);
                }
                other => return Err(format!("unknown build option `{other}`\n\n{}", help())),
            }
            at += 1;
        }
        if !static_mode {
            return Err(format!(
                "Rahti currently builds static output with `--static`.\n\n{}",
                help()
            ));
        }
        Ok(Self { output })
    }
}

pub fn run(args: &[&str]) -> Result<(), String> {
    if args.iter().any(|arg| matches!(*arg, "--help" | "-h")) {
        println!("{}", help());
        return Ok(());
    }
    let options = Options::parse(args)?;
    let manifest = fs::read_to_string("Cargo.toml")
        .map_err(|error| format!("cannot read Cargo.toml: {error}"))?;
    let package =
        package_name(&manifest).ok_or_else(|| "Cargo.toml has no `[package]` name".to_string())?;
    let main = fs::read_to_string("src/main.rs")
        .map_err(|error| format!("cannot read src/main.rs: {error}"))?;
    if !main.contains("static_output()") {
        return Err(
            "src/main.rs does not contain Rahti's static-export handoff.\n  \
             Run `cargo rahti upgrade --force src/main.rs`, or add the handoff \
             documented in `docs/conventions/static-generation.md`."
                .to_string(),
        );
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        // Select the application package and let Cargo select its sole or
        // default binary. The framework workspace deliberately calls that
        // binary `rahti` while its package is `rahti-app`; assuming both names
        // match would make the tool work only in freshly scaffolded projects.
        .args(["run", "--package", package])
        .env(OUTPUT_ENV, &options.output)
        // Static output must not carry the development reload client, even
        // though a debug compile keeps first-run auth secrets convenient.
        .env("RAHTI_DEV", "0")
        .status()
        .map_err(|error| format!("cannot run the application exporter: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("the application exporter exited with {status}"))
    }
}

fn package_name(manifest: &str) -> Option<&str> {
    let mut package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            package = line == "[package]";
            continue;
        }
        if package {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "name" {
                let value = value.trim();
                return value.strip_prefix('"')?.strip_suffix('"');
            }
        }
    }
    None
}

fn help() -> &'static str {
    "USAGE:\n    cargo rahti build --static [--out <directory>]\n\n\
     Builds CSS, starts the application in-process without a listener, renders\n\
     its declared pages, and copies public assets. The default output is dist/."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_build_defaults_to_dist() {
        let options = Options::parse(&["--static"]).unwrap();
        assert_eq!(options.output, PathBuf::from("dist"));
    }

    #[test]
    fn output_can_be_named() {
        let options = Options::parse(&["--static", "--out", "site"]).unwrap();
        assert_eq!(options.output, PathBuf::from("site"));
    }

    #[test]
    fn build_mode_is_explicit() {
        assert!(Options::parse(&[]).unwrap_err().contains("--static"));
    }

    #[test]
    fn reads_only_the_package_name() {
        let manifest = "[workspace]\nname = \"wrong\"\n[package]\nname = \"right\"\n";
        assert_eq!(package_name(manifest), Some("right"));
    }
}
