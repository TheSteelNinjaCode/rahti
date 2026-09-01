//! `cargo rahti studio` — open Prisma Studio for a Rahti application's database.
//!
//! SeaORM deliberately has no bundled database browser. Prisma Studio can
//! inspect the same SQLite, PostgreSQL, or MySQL database directly, so this
//! command is a small bridge rather than a second ORM or an application route.
//! It reads the same `DATABASE_URL` precedence the Rahti runtime uses: an
//! exported value wins; otherwise the application's ignored `.env` supplies it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
#[path = "tests/studio.rs"]
mod tests;

/// Start Prisma Studio from the current Rahti project.
pub fn run(args: &[&str]) -> Result<(), String> {
    let options = Options::parse(args)?;
    if options.help {
        print!("{}", usage());
        return Ok(());
    }

    let root =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    ensure_database_project(&root)?;
    let url = database_url(&root, std::env::var("DATABASE_URL").ok())?;
    let url = prisma_url(&url)?;
    let invocation = Invocation::new(&root, url, options.port);

    let status = Command::new(invocation.program)
        .args(&invocation.args)
        .current_dir(&invocation.cwd)
        .status()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "cannot find `npx` on PATH: {e}\n\
                 Install Node.js and Prisma with:\n    npm install --save-dev prisma"
            ),
            _ => format!("could not start Prisma Studio: {e}"),
        })?;

    if status.success() {
        Ok(())
    } else {
        Err("Prisma Studio stopped without starting successfully. Ensure Prisma is installed in this project with:\n    npm install --save-dev prisma".to_string())
    }
}

#[derive(Default)]
struct Options {
    port: Option<u16>,
    help: bool,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut rest = args.iter().copied();
        while let Some(arg) = rest.next() {
            match arg {
                "--port" => {
                    let raw = rest.next().ok_or("`--port` needs a port number")?;
                    let port: u16 = raw
                        .parse()
                        .map_err(|_| format!("`{raw}` is not a port number"))?;
                    if port == 0 {
                        return Err("`--port` must be between 1 and 65535".to_string());
                    }
                    options.port = Some(port);
                }
                "--help" | "-h" => options.help = true,
                other => return Err(format!("`{other}` is not a cargo-rahti studio option")),
            }
        }
        Ok(options)
    }
}

struct Invocation {
    program: &'static str,
    args: Vec<String>,
    cwd: PathBuf,
}

impl Invocation {
    fn new(root: &Path, url: String, port: Option<u16>) -> Self {
        let mut args = vec![
            "--no-install".to_string(),
            "prisma".to_string(),
            "studio".to_string(),
            "--url".to_string(),
            url,
        ];
        if let Some(port) = port {
            args.push("--port".to_string());
            args.push(port.to_string());
        }
        Self {
            program: "npx",
            args,
            cwd: root.to_path_buf(),
        }
    }
}

fn ensure_database_project(root: &Path) -> Result<(), String> {
    let path = root.join("rahti.config.json");
    let raw = fs::read_to_string(&path).map_err(|_| {
        "`cargo rahti studio` must run from a Rahti project directory containing rahti.config.json"
            .to_string()
    })?;
    let config: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("cannot read rahti.config.json: {e}"))?;
    if !config.get("db").is_some_and(serde_json::Value::is_object) {
        return Err("this Rahti project has no configured database. Add one with `cargo rahti upgrade --db` first".to_string());
    }
    Ok(())
}

fn database_url(root: &Path, inherited: Option<String>) -> Result<String, String> {
    if let Some(url) = inherited.filter(|url| !url.trim().is_empty()) {
        return Ok(url);
    }
    let env = fs::read_to_string(root.join(".env")).map_err(|_| {
        "DATABASE_URL is not set and this project has no readable .env file".to_string()
    })?;
    parse_database_url(&env)
        .ok_or_else(|| "DATABASE_URL is not set in the environment or .env".to_string())
}

fn parse_database_url(env: &str) -> Option<String> {
    env.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (name, value) = line.split_once('=')?;
        if name.trim() != "DATABASE_URL" {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Prisma's SQLite connector uses `file:` while Rahti's SeaORM scaffold uses
/// `sqlite://`; remove SeaORM-only query settings during that translation.
fn prisma_url(database_url: &str) -> Result<String, String> {
    if let Some(path) = database_url.strip_prefix("sqlite://") {
        let path = path.split_once('?').map_or(path, |(path, _)| path);
        if path.is_empty() || path == ":memory:" {
            return Err("Prisma Studio needs a file-backed SQLite DATABASE_URL".to_string());
        }
        return Ok(format!("file:{path}"));
    }
    if let Some(rest) = database_url.strip_prefix("postgres://") {
        return Ok(format!("postgresql://{rest}"));
    }
    if database_url.starts_with("postgresql://") || database_url.starts_with("mysql://") {
        return Ok(database_url.to_string());
    }
    Err(
        "DATABASE_URL must name a SQLite, PostgreSQL, or MySQL database supported by Prisma Studio"
            .to_string(),
    )
}

fn usage() -> &'static str {
    "cargo rahti studio [--port <port>]\n\
\n\
Open Prisma Studio for this Rahti project's configured database. DATABASE_URL\n\
comes from the environment first, then the ignored .env file; its value is\n\
never printed. Prisma must already be installed in the project:\n\
\n\
    npm install --save-dev prisma\n\
\n\
OPTIONS:\n\
    --port <port>     Port for Prisma Studio's local server.\n\
    -h, --help        Print this message.\n"
}
