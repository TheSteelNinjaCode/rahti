//! `cargo rahti restart`, `stop` and `status`.
//!
//! The apply step of working on a Rahti project. A coding agent cannot press
//! Ctrl+C and cannot see a terminal it did not open, and nothing it edits is
//! real until the running server is replaced, so this does the whole of that
//! and answers with one line it can act on.
//!
//! Order is what makes a failed edit safe: the source is type-checked while
//! the old server still runs, so a compile error costs nothing and is reported
//! as "still running old code". Only then is the server stopped, because
//! Windows refuses to overwrite a running executable.
//!
//! The new server has to outlive this command, and the shell that ran it.
//! On Windows an agent's tool runs each command inside a job object that kills
//! everything in it when the tool's process tree ends, so the server is
//! created through WMI, which puts it in no job.

use rahti_build::dev_server::{self, SERVER_LOG_PATH, ServerInfo};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const LAUNCH_SPEC: &str = ".rahti/dev-launch.json";
const LAUNCHED_PID: &str = ".rahti/dev-launch.pid";
const POLL: Duration = Duration::from_millis(100);

fn say(message: &str) {
    println!("[rahti] {message}");
}

fn project_root() -> Result<PathBuf, String> {
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    if root.join("rahti.config.json").is_file() {
        return Ok(root);
    }
    Err(format!(
        "{} is not a Rahti project: rahti.config.json is missing. Run this from the \
         project root.",
        root.display()
    ))
}

fn timeout_from(args: &[&str]) -> Result<Duration, String> {
    let mut seconds = DEFAULT_TIMEOUT_SECS;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--timeout" => {
                seconds = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or("`--timeout` takes a number of seconds")?;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Duration::from_secs(seconds))
}

pub fn run_restart(args: &[&str]) -> Result<(), String> {
    let timeout = timeout_from(args)?;
    let root = project_root()?;
    let running = running_server(&root);

    if let Some(info) = &running {
        say("Checking the source while the running server stays up.");
        if !cargo(&root, &["check"])? {
            return Err(format!(
                "the source does not compile. {} is still running the old code.",
                info.url
            ));
        }
        stop_server(&root, info, timeout)?;
    }

    say("Building.");
    let executable = build(&root)?;
    let previous = dev_server::read_info(&root).map(|info| info.boot);
    let info = start(&root, &executable, previous.as_deref(), timeout)?;

    say(&format!(
        "Ready at {} (pid {}, boot {}, output in {SERVER_LOG_PATH})",
        info.url, info.pid, info.boot
    ));
    say(&format!(
        "Next: load {}<route> for each affected route, exercise the change, then run \
         `cargo rahti status`.",
        info.url
    ));
    Ok(())
}

pub fn run_stop(args: &[&str]) -> Result<(), String> {
    let timeout = timeout_from(args)?;
    let root = project_root()?;
    match running_server(&root) {
        Some(info) => stop_server(&root, &info, timeout),
        None => {
            say("No development server is running.");
            Ok(())
        }
    }
}

/// Exits non-zero unless the server is up and running the current source, so
/// a script can branch on it without reading the words.
pub fn run_status(args: &[&str]) -> Result<(), String> {
    let root = project_root()?;
    let json_output = match args {
        [] => false,
        ["--json"] => true,
        [other, ..] => return Err(format!("unknown option `{other}`")),
    };

    let mut report = dev_server::status(&root);
    report["mcpMissing"] = json!(missing_mcp_commands(&root));
    if json_output {
        println!("{report}");
    } else {
        print!("{}", describe(&report));
    }
    if report["running"] == true && report["stale"] == false {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

/// The report as an agent or a person reads it in a terminal.
pub(crate) fn describe(report: &Value) -> String {
    let mut out = String::new();
    let line = |out: &mut String, text: String| {
        out.push_str(&text);
        out.push('\n');
    };

    if report["running"] != true {
        line(&mut out, "Server: not running".to_string());
        mcp_line(&mut out, report);
        line(&mut out, format!("Next: {}", text(report, "next")));
        return out;
    }

    line(
        &mut out,
        format!(
            "Server: running at {} (pid {}, boot {})",
            text(report, "url"),
            report["pid"],
            text(report, "boot")
        ),
    );
    let changed = list(&report["changedFiles"]);
    if changed.is_empty() {
        line(&mut out, "Source: current".to_string());
    } else {
        line(
            &mut out,
            format!("Source: STALE, changed since boot: {}", changed.join(", ")),
        );
    }

    let browser = &report["browser"];
    let routes = browser["routes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if routes.is_empty() {
        line(
            &mut out,
            "Browser: no page loaded under this boot, so nothing is verified".to_string(),
        );
    } else {
        line(
            &mut out,
            format!(
                "Browser: {} route(s) loaded, {} error(s), {} warning(s)",
                routes.len(),
                browser["errors"],
                browser["warnings"]
            ),
        );
        for route in routes {
            line(
                &mut out,
                format!(
                    "  {} loads={} errors={} warnings={}",
                    text(route, "route"),
                    route["loads"],
                    route["errors"],
                    route["warnings"]
                ),
            );
        }
        for problem in browser["problems"].as_array().into_iter().flatten() {
            line(
                &mut out,
                format!(
                    "  {} on {}: {}",
                    text(problem, "level"),
                    text(problem, "route"),
                    text(problem, "message")
                ),
            );
        }
    }

    let jobs = &report["jobs"];
    if jobs.is_object() {
        let store = if jobs["durable"] == true {
            "stored"
        } else {
            "in memory"
        };
        let mut summary = format!(
            "Jobs: {} handler(s), {store}, {} succeeded, {} failed",
            jobs["handlers"].as_array().map_or(0, Vec::len),
            jobs["metrics"]["succeeded"],
            jobs["metrics"]["failed"],
        );
        if jobs["stored"].is_object() {
            summary.push_str(&format!(
                ", {} pending, {} running",
                jobs["stored"]["pending"], jobs["stored"]["running"]
            ));
        }
        line(&mut out, summary);
        if !jobs["storeError"].is_null() {
            line(
                &mut out,
                format!("  store error: {}", text(jobs, "storeError")),
            );
        }
        for failure in jobs["recentFailures"].as_array().into_iter().flatten() {
            line(
                &mut out,
                format!(
                    "  {} {} after {} attempt(s): {}",
                    text(failure, "name"),
                    text(failure, "outcome"),
                    failure["attempt"],
                    text(failure, "error")
                ),
            );
        }
    }

    mcp_line(&mut out, report);
    line(&mut out, format!("Next: {}", text(report, "next")));
    out
}

/// An MCP server `.mcp.json` starts but this machine cannot: the client runs
/// the command, it is not found, and the agent sees only a closed connection.
fn mcp_line(out: &mut String, report: &Value) {
    for command in list(&report["mcpMissing"]) {
        let install = if command == "rahti-mcp" {
            format!(
                " Install it with `cargo install rahti-mcp --version {}`.",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "MCP: .mcp.json starts `{command}`, which is not installed, so MCP clients \
             cannot connect.{install}\n"
        ));
    }
}

/// The commands `.mcp.json` names that are not on `PATH`.
pub(crate) fn missing_mcp_commands(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join(".mcp.json")) else {
        return Vec::new();
    };
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut missing = Vec::new();
    for server in config["mcpServers"].as_object().into_iter().flatten() {
        let Some(command) = server.1["command"].as_str() else {
            continue;
        };
        if !installed(command, &path) && !missing.iter().any(|known| known == command) {
            missing.push(command.to_string());
        }
    }
    missing
}

/// Whether `command` resolves the way a client starting it would resolve it:
/// a path as written, otherwise a `PATH` entry, with Windows' executable
/// extensions.
pub(crate) fn installed(command: &str, path: &std::ffi::OsStr) -> bool {
    let named = Path::new(command);
    if named.components().count() > 1 {
        return named.is_file();
    }
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(path).any(|dir| {
        extensions
            .iter()
            .any(|extension| dir.join(format!("{command}{extension}")).is_file())
    })
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn list(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn running_server(root: &Path) -> Option<ServerInfo> {
    let info = dev_server::read_info(root)?;
    dev_server::live_status(&info).map(|_| info)
}

/// Ask for the graceful stop and wait until the address stops answering.
fn stop_server(root: &Path, info: &ServerInfo, timeout: Duration) -> Result<(), String> {
    match dev_server::request_shutdown(info) {
        Ok(202) => {}
        Ok(status) => {
            return Err(format!(
                "the server at {} refused the graceful stop (HTTP {status}). Stop it once \
                 with Ctrl+C in its terminal.",
                info.url
            ));
        }
        Err(error) => return Err(format!("cannot reach {}: {error}", info.url)),
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if dev_server::live_status(info).is_none() && process_gone(info.pid) {
            // A crash leaves this file behind, and the next start reads it
            // to tell its own boot from the last one.
            let _ = fs::remove_file(root.join(dev_server::INFO_PATH));
            say(&format!("Stopped the server at {}.", info.url));
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
    Err(format!(
        "the server at {} did not finish shutting down within {}s. Jobs may still be \
         draining: run `cargo rahti status`, and do not force-kill it.",
        info.url,
        timeout.as_secs()
    ))
}

fn cargo(root: &Path, args: &[&str]) -> Result<bool, String> {
    let status = Command::new(cargo_program())
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    Ok(status.success())
}

fn cargo_program() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// Build the project's binary and return the path cargo reports for it.
///
/// Rendered diagnostics go to standard error as usual, and standard output
/// carries the JSON that names the artifact, so a compile error reads the same
/// as `cargo build` while the path is never guessed.
fn build(root: &Path) -> Result<PathBuf, String> {
    let mut child = Command::new(cargo_program())
        .args(["build", "--message-format=json-render-diagnostics"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run cargo: {error}"))?;

    let stdout = child.stdout.take().ok_or("cargo gave no output stream")?;
    let mut executable = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(path) = artifact_executable(&line) {
            executable = Some(path);
        }
    }

    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("the build failed, and no server is running now.".to_string());
    }
    executable.ok_or_else(|| "cargo built no executable: is this a binary project?".to_string())
}

fn artifact_executable(line: &str) -> Option<PathBuf> {
    let message: Value = serde_json::from_str(line).ok()?;
    if message["reason"] != "compiler-artifact" {
        return None;
    }
    let kinds = message["target"]["kind"].as_array()?;
    if !kinds.iter().any(|kind| kind == "bin") {
        return None;
    }
    message["executable"].as_str().map(PathBuf::from)
}

/// Start `executable` detached and wait for a boot that is not `previous`.
fn start(
    root: &Path,
    executable: &Path,
    previous: Option<&str>,
    timeout: Duration,
) -> Result<ServerInfo, String> {
    let log = root.join(SERVER_LOG_PATH);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let mut direct = None;
    let _ = fs::remove_file(root.join(LAUNCHED_PID));
    if !launch_outside_job(root, executable, &log) {
        direct = Some(spawn_detached(root, executable, &log, &environment())?);
    }

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if server_exited(root, &mut direct) {
            return Err(format!(
                "the server exited during startup. Its output:\n{}",
                tail(&log)
            ));
        }
        if let Some(info) = dev_server::read_info(root)
            && Some(info.boot.as_str()) != previous
            && dev_server::live_status(&info).is_some()
        {
            return Ok(info);
        }
        std::thread::sleep(POLL);
    }
    Err(format!(
        "the server did not become ready within {}s. Is RAHTI_DEV turned off? Output so \
         far:\n{}",
        timeout.as_secs(),
        tail(&log)
    ))
}

fn server_exited(root: &Path, direct: &mut Option<Child>) -> bool {
    if let Some(child) = direct {
        return matches!(child.try_wait(), Ok(Some(_)));
    }
    fs::read_to_string(root.join(LAUNCHED_PID))
        .ok()
        .and_then(|pid| pid.trim().parse().ok())
        .is_some_and(process_gone)
}

fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines = text.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

fn environment() -> Vec<(String, String)> {
    std::env::vars()
        // The dev server's own build-tool variables mean nothing to it, and a
        // fresh process must not inherit `cargo run`'s view of the world.
        .filter(|(name, _)| !name.starts_with("CARGO"))
        .collect()
}

fn spawn_detached(
    root: &Path,
    executable: &Path,
    log: &Path,
    environment: &[(String, String)],
) -> Result<Child, String> {
    let output = fs::File::create(log).map_err(|error| error.to_string())?;
    let errors = output.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors);
    detach(&mut command);
    command
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", executable.display()))
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Start this tool again through WMI, which then starts the server: a process
/// WMI creates belongs to no job object, and so does what it starts. The
/// environment travels in a short-lived file because WMI passes none, and the
/// launcher deletes that file as soon as it has read it.
#[cfg(windows)]
fn launch_outside_job(root: &Path, executable: &Path, log: &Path) -> bool {
    let Ok(tool) = std::env::current_exe() else {
        return false;
    };
    let spec = json!({
        "executable": executable,
        "log": log,
        "environment": environment(),
    });
    let spec_path = root.join(LAUNCH_SPEC);
    if fs::write(&spec_path, spec.to_string()).is_err() {
        return false;
    }

    let quote = |value: String| format!("'{}'", value.replace('\'', "''"));
    let command_line = format!("\"{}\" __launch", tool.display());
    let script = format!(
        "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments \
         @{{CommandLine={}; CurrentDirectory={}}}; $r.ReturnValue",
        quote(command_line),
        quote(root.display().to_string()),
    );
    let launched = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "0"
        });
    if !launched {
        let _ = fs::remove_file(&spec_path);
    }
    launched
}

#[cfg(not(windows))]
fn launch_outside_job(_root: &Path, _executable: &Path, _log: &Path) -> bool {
    false
}

/// `__launch`: the half of a Windows start that runs outside the job object.
pub fn run_launch() -> Result<(), String> {
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    let spec_path = root.join(LAUNCH_SPEC);
    let text = fs::read_to_string(&spec_path).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&spec_path);
    let spec: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;

    let environment = spec["environment"]
        .as_array()
        .ok_or("the launch spec has no environment")?
        .iter()
        .filter_map(|pair| Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string())))
        .collect::<Vec<_>>();
    let executable = spec["executable"]
        .as_str()
        .ok_or("the launch spec has no executable")?;
    let log = spec["log"].as_str().ok_or("the launch spec has no log")?;

    let child = spawn_detached(&root, Path::new(executable), Path::new(log), &environment)?;
    fs::write(root.join(LAUNCHED_PID), child.id().to_string()).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn process_gone(pid: u64) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .is_ok_and(|output| {
            !String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
        })
}

#[cfg(unix)]
fn process_gone(pid: u64) -> bool {
    !Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
#[path = "tests/dev_server.rs"]
mod tests;
