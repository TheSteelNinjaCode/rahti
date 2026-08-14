//! The server's listener.
//!
//! ## Where the address comes from
//!
//! `rahti.config.json` records the address a project develops on, and the
//! generated `routes.rs` hands that pair to [`listen`] as `HOST` and `PORT`.
//! A managed platform does not read that file: it starts the built binary and
//! names the address through the environment, usually assigning a different
//! port on every deploy. So the environment wins — `PORT` overrides the
//! configured port and `HOST` overrides the configured host, both read through
//! [`crate::load_env`], so a `.env` sets them locally too.
//!
//! That is the whole deployment story for the address: a project develops on
//! the address it was scaffolded with and deploys with no edit to `main.rs`,
//! no second copy of the port in `rahti.config.json`, and no platform-specific
//! branch. A variable exported empty counts as unset — a platform that writes
//! `PORT=` has not chosen a port.
//!
//! `HOST` is an override rather than a requirement, because the scaffolded
//! host is already the wildcard: `cargo rahti new` writes `0.0.0.0`, the one
//! address that answers both on this machine and to whatever routes traffic
//! into a container. A project that narrowed it to the loopback is the one
//! that needs `HOST` set in a deployment.
//!
//! ## When the port is taken
//!
//! Binding's overwhelmingly common failure — `AddrInUse` — has two correct
//! answers depending on who is running:
//!
//! - In development the culprit is a server from an earlier run still holding
//!   the port, and the developer's goal is a running server, not that exact
//!   port. So dev mode walks forward to the next free port and says so; the
//!   `Server running on` line that follows names the address that actually
//!   bound. This holds however the port was chosen: a developer who sets
//!   `PORT` in a `.env` still gets a server.
//! - In production the port is a contract — a platform's router or a reverse
//!   proxy points at it — so moving off it silently would break the deployment
//!   in a way nothing reports. A release build states the conflict, names
//!   where the port came from, and exits.
//!
//! Dev mode is [`crate::dev::enabled`]: the build profile, overridden either
//! way by `RAHTI_DEV`.
//!
//! A moved port does not disturb CSRF: the token cookie's name is derived per
//! request from the browser's own `Host` header, not from the configured port.

use std::io::ErrorKind;
use std::net::SocketAddr;

use tokio::net::TcpListener;

#[cfg(test)]
#[path = "tests/listen.rs"]
mod tests;

/// Overrides the configured port. The name every managed platform sets.
pub const PORT_ENV: &str = "PORT";

/// Overrides the configured host. A platform that expects a specific bind
/// address says so here; most only need the default `0.0.0.0` a container
/// image already uses.
pub const HOST_ENV: &str = "HOST";

/// Bind the address the environment names, or the one `routes.rs` configures.
///
/// Returns the listener, or exits the process with an explanation — the same
/// contract the rest of startup (`db::connect`, the auth policy) holds to: a
/// server that cannot start says why once, without a backtrace.
pub async fn listen(host: &str, port: u16) -> TcpListener {
    // Both variables may come from a `.env`, so the file has to be in the
    // environment before either is read. Idempotent, and startup calls it
    // from `db::connect` and the auth policy too.
    crate::load_env();

    let host = from_env(HOST_ENV).unwrap_or_else(|| host.to_string());
    let (port, source) = match from_env(PORT_ENV) {
        Some(raw) => (parse_port(&raw), PortSource::Environment),
        None => (port, PortSource::Config),
    };

    let addr = format!("{host}:{port}");
    match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            if crate::dev::enabled() {
                if let Some(listener) = next_free(&host, port).await {
                    let moved = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                    println!("rahti dev: {addr} is in use — using port {moved} instead.");
                    return listener;
                }
            }
            eprintln!(
                "rahti: {addr} is already in use.\n  \
                 Another server is still running — stop it, or change {}.",
                source.describe()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("rahti: cannot listen on {addr}: {e}");
            std::process::exit(1);
        }
    }
}

/// Which of the two answers to "change the port" applies, so a conflict tells
/// the reader where the value it is complaining about actually lives.
enum PortSource {
    Config,
    Environment,
}

impl PortSource {
    fn describe(&self) -> String {
        match self {
            Self::Config => "`server.port` in rahti.config.json".to_string(),
            Self::Environment => format!("the {PORT_ENV} environment variable"),
        }
    }
}

/// A variable's value, with a blank one counted as unset.
fn from_env(name: &str) -> Option<String> {
    present(std::env::var(name).ok())
}

/// The value a variable carries, or `None` for absent, blank, or whitespace.
///
/// Separate from the reading so it can be tested without touching the
/// process's own environment, which every other test shares.
fn present(raw: Option<String>) -> Option<String> {
    let value = raw?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// `PORT` as a port number, or an exit — a platform handing us something that
/// is not a port is a misconfiguration to report, not a value to guess at.
///
/// `0` is honored rather than rejected: it asks the OS for a free port, which
/// is what a test harness wants.
fn parse_port(raw: &str) -> u16 {
    raw.parse::<u16>().unwrap_or_else(|_| {
        eprintln!(
            "rahti: {PORT_ENV}={raw} is not a TCP port.\n  \
             Expected a number from 0 to 65535."
        );
        std::process::exit(1);
    })
}

/// Where to open a browser, for a listener bound anywhere.
///
/// A bind address and a destination are not the same thing, and `0.0.0.0` is
/// only the first: it means "every interface on this machine", which is a
/// sentence about listening, not somewhere to send a request. A browser handed
/// it says so — Chrome refuses `http://0.0.0.0:3000` with
/// `ERR_ADDRESS_INVALID` — so a startup line that printed the bind address
/// verbatim gave the developer a URL that cannot be clicked, on exactly the
/// configuration a deployment needs.
///
/// Which is the configuration people end up developing on, because a platform
/// reaches the process from outside the container and `127.0.0.1` refuses it.
/// So the wildcard is printed as `localhost`: the same server, named the way
/// this machine can reach it. Any other address is already a destination and
/// is printed as it is.
pub fn local_url(listener: &TcpListener) -> String {
    url_of(
        listener
            .local_addr()
            .expect("a bound listener has an address"),
    )
}

/// The printing half, separated from the socket so it can be tested over the
/// addresses that matter rather than over whatever this machine will bind.
fn url_of(addr: SocketAddr) -> String {
    // Both wildcards — `0.0.0.0` and `[::]` — answer on the loopback the
    // browser is about to use.
    if addr.ip().is_unspecified() {
        return format!("http://localhost:{}", addr.port());
    }
    // `SocketAddr`'s own formatting brackets IPv6, which is what a URL wants.
    format!("http://{addr}")
}

/// The first port after `port` that accepts a bind.
///
/// Increments until something is free, so a run of stranded servers is walked
/// straight past — each busy port costs one refused bind, nothing more. The
/// scan ends where ports do, at 65535; it never wraps back below the
/// configured port, because ports below it belong to other deliberate
/// configurations.
async fn next_free(host: &str, port: u16) -> Option<TcpListener> {
    for candidate in port.checked_add(1)?..=u16::MAX {
        if let Ok(listener) = TcpListener::bind((host, candidate)).await {
            return Some(listener);
        }
    }
    None
}
