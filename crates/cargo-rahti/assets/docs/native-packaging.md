# Native Packaging

Optional. A Rahti project is a web application, and stays one. Nothing in this
document applies until you run `cargo rahti native init`, and a project that
never does compiles, downloads and depends on none of it.

## What a native package is

The application you already have, running inside an installed program.

Your Rust backend is compiled for the target platform and runs in the packaged
process. The generated Axum router — the same one, with the same auth guard,
the same CSRF layer and the same static fallback — answers on a loopback
socket. The operating system's WebView loads it: WebView2 on Windows, Android
System WebView on Android.

So your pages, components, layouts, error boundaries, loading regions, rpcs,
streaming rpcs, uploads, sockets, sessions and PulsePoint bindings all work,
because none of them are involved. There is nothing to convert and no
compatibility subset.

**It is not an HTML-to-native-widget compiler.** Nothing here translates
markup. What the user sees is a WebView, and the controls in it are the
browser's, not the platform's.

## Prerequisites

Packaging is a separate install, which is what makes it optional:

```text
cargo install cargo-rahti-native
```

It delegates the packaging itself to Tauri's own CLI, which is another:

```text
cargo install tauri-cli --version "^2.8" --locked
```

Two tools rather than one because the Tauri CLI has to match the `tauri`
version in your `native/Cargo.toml`, which is yours to upgrade on your own
schedule.

Android additionally needs the Android SDK, the NDK, a JDK and four Rust
targets. `cargo rahti native doctor` names each missing one and the command
that supplies it. It installs nothing itself.

## The commands

```text
cargo rahti native init --identifier com.example.myapp --windows --android
cargo rahti native doctor
cargo rahti native dev   --target windows
cargo rahti native dev   --target android
cargo rahti native build --target windows
cargo rahti native build --target android --format apk
cargo rahti native build --target android --format aab
```

`cargo rahti-native …` reaches the same tool directly, through cargo's own
subcommand dispatch. If the helper is not installed, `cargo rahti native`
prints the one line that installs it and does nothing else.

`build` prints the absolute path of every artifact it produced.

## Development reload

`dev` stays running and watches the application as well as the generated
native shell. What happens after a save depends on what changed and where the
application is running:

| Change | Windows | Android |
| --- | --- | --- |
| `src/`, `Cargo.toml`, `Cargo.lock`, `build.rs`, `rahti.config.json`, `.env` | Rebuild and restart the native application. | Rebuild, reinstall and restart it on the selected device. |
| `public/*.css` | Swap the stylesheet in the existing WebView. | Rebuild, reinstall and restart with new embedded assets. |
| Another file under `public/` | Reload the existing WebView. | Rebuild, reinstall and restart with new embedded assets. |

Rust is compiled into the application, so this is restart-based live reload,
not state-preserving hot-module replacement. Windows asset edits are the fast
case because the debug process can serve the checkout directly. Android
cannot read a directory on the development machine: its assets have to cross
the package boundary, so the automatic loop takes as long as an incremental
Android build and deployment.

The application is outside `native/`'s deliberately separate Cargo workspace.
Tauri CLI 2.8 introduced the additional watch folders needed to observe it;
`doctor` refuses an older CLI and prints the upgrade command.

An existing project gets this behavior after updating `cargo-rahti-native` and
running `cargo rahti native init` again. Untouched generated shell files are
updated; an application-owned file you edited remains yours and is reported,
as usual.

## Your application has to expose its startup

The native shell starts the same application the web binary starts, by calling
one function in your library:

```rust
// src/lib.rs
pub async fn initialize_application() -> Result<ApplicationRuntime, StartupError>
```

`src/main.rs` calls it and binds the public listener. The shell calls it and
binds a loopback one. Nothing about connecting a database, applying migrations
or installing an auth policy is duplicated, and nothing about it is the
framework's — which migrations run and when is a decision your `src/lib.rs`
makes.

The split exists because Android has no `main`: the operating system loads a
library and calls into it. A project whose startup is still inside
`fn main()` cannot be packaged, and `doctor` says so with the command that
fixes it.

**Startup returns its errors rather than exiting on them.** A packaged process
that calls `std::process::exit` during startup is, from the user's side, a
program that did not open. The web binary prints and exits; the shell shows
the same sentence in a dialog.

## `rahti.native.json`

Written by `init`, beside `rahti.config.json`, with a schema file next to it.

```json
{
  "$schema": "./rahti.native.schema.json",
  "schema": 1,
  "productName": "My App",
  "identifier": "com.example.myapp",
  "version": "0.1.0",
  "targets": ["windows", "android"],
  "window": { "title": "My App", "width": 1200, "height": 800 },
  "android": { "minSdk": 24 },
  "database": { "mode": "sqlite-local" },
  "security": { "loopbackToken": true }
}
```

It holds names, sizes and identifiers. It holds **no credentials**, and there
is no field to put one in: signing is configured through the environment, and
the session key is generated on the device.

The identifier is validated to Android's rule, which is the strict one: at
least two segments, each a legal Java identifier, **no hyphens**.
`com.example.my-app` is a perfectly good Windows bundle identity and stops a
Gradle build several minutes in, so it is refused before anything is written.

`auth.cookieName` is recorded so a package keeps the same session cookie name
as your web deployment. A cookie name is not a credential — it is in every
response header the application sends. `AUTH_SECRET` is not read, not written
and not stored.

Editing this file and re-running `init` is how the shell learns a new window
size, version or target.

## `native/` is yours

`init` writes a Tauri shell and then stays out of it. It records a hash of each
generated file; a file whose hash still matches is regenerated, and one you
have **edited** is left alone and reported. `--force` takes an edited file
back, and does not keep what it replaces.

The shell is a separate cargo package, deliberately outside your workspace
(`exclude = ["native"]` in the project's `Cargo.toml`). It is the only package
in a Rahti project that depends on Tauri, so `cargo check --workspace`,
`cargo test --workspace` and `cargo build` never resolve a native dependency.

The generated icons are the **Rahti mark** — a real icon set, at every size
Windows and Android ask for, so a first build looks like an application rather
than a blank square. It is still not *your* mark: replace it with

```bash
cargo tauri icon path/to/icon.png
```

which rewrites the whole set from one square PNG.

The Android half matters more than it looks. `cargo tauri android init` writes
a project whose `res/mipmap-*` already holds the project template's own
launcher icon, and nothing copies `native/icons/` over it — so an Android build
would wear the template's robot no matter what is in `icons/`. `cargo rahti
native` copies the icons in on every Android build, and removes the template's
file for the same resource name, because two files claiming one name is an
`aapt2` failure rather than a preference.

## Where a packaged application keeps things

Installed applications cannot assume the repository root is the working
directory, and cannot write to their installation directory at all.

| | Windows | Android |
| --- | --- | --- |
| data | `%LOCALAPPDATA%\<identifier>` | internal files directory |
| config | `%APPDATA%\<identifier>` | `<files>/config` |
| cache | `%LOCALAPPDATA%\<identifier>\cache` | cache directory |

The database, uploads, logs and the session key are **data**. Spilled upload
parts and scratch files are **cache**, because the operating system may delete
a cache directory when the device is short of space and a spilled part exists
for one request.

`RAHTI_PUBLIC_DIR` and `RAHTI_SPILL_DIR` are set to the resolved paths before
the router is built.

### Static assets

**Your `public/` is compiled into the executable**, and written out to
application storage on first launch.

That is not the obvious design, and the obvious one does not work. Tauri's
bundler does copy declared resources into an Android package — into the APK's
`assets/`, which is a zip entry rather than a file — and `resource_dir()` on
Android returns the string `asset://localhost/`. It is a URI, not a directory:
`ServeDir` pointed at it serves nothing. An application built that way starts,
binds its port, opens its window, and 404s every stylesheet and the whole
PulsePoint runtime. Reading the zip instead would mean the Android
AssetManager, which means JNI in what is otherwise platform-neutral code.

Embedding sidesteps all of it, and one code path then serves both platforms.
It costs the size of `public/` in the binary.

An upgrade replaces the staged tree whole — a stale `pp-reactive-v2.min.js`
beside a current `main.js` fails in ways nothing explains — and touches nothing
else: the staged tree is a subdirectory of the data directory, and the
database, the uploads and the key are its siblings.

A **Windows debug** build serves your project's own `public/` directly, so a
stylesheet edit is visible without a rebuild. Android cannot reach that host
path; its debug build uses the embedded tree and `native dev` rebuilds and
redeploys it after an asset edit. A content fingerprint replaces the staged
tree even when the application version did not change. A release package never
looks at the host path and so never depends on the machine that built it.

### Android and cleartext

Android refuses cleartext HTTP by default, and Tauri's generated project sets
`usesCleartextTraffic="false"` for release builds — right for Tauri, which
serves through an asset protocol, and wrong for Rahti, which serves itself over
`http://127.0.0.1`.

So `cargo rahti native` writes a network security config into the generated
Android project and points the manifest at it. Cleartext stays refused for
every host except the loopback addresses the embedded server binds — narrower
than flipping the manifest flag, which would have permitted it everywhere.

It is reapplied on every Android build, because `gen/android` is regenerated
rather than reviewed.

### The database

`database.mode` decides, and the default is `sqlite-local`: a SQLite file in
application storage, reached through an absolute `DATABASE_URL` created on
first launch.

A project on PostgreSQL or MySQL sets `remote`, and its `DATABASE_URL` is left
exactly as it is. Rewriting it to a local SQLite file would start the
application against an empty database that looked like a working one. A
packaged application is a client of a remote database, and that is the whole
of what native packaging does about it.

## The session key

`rahti::auth` signs its session cookie with `AUTH_SECRET`. A packaged
application has no `.env` to read one from, and must not have one: a key
shipped inside an installer is a key every installation can forge every other
installation's sessions with. Going without is not an option either —
`rahti::auth` invents one per process, so everyone is signed out at every
restart.

So the key is generated **on the device at first launch** and kept in
application storage, per installation.

- **Windows**: encrypted with DPAPI before it is written. The blob is readable
  by that user account on that machine and by nobody else.
- **Android**: in the internal files directory, which is the platform's own
  per-application sandbox. Keystore-backed encryption on top of that needs
  Kotlin and a plugin, and this is not one.

A key file that cannot be decrypted — a restored profile, a recreated account
— is replaced rather than fatal. The user is signed out; the application opens.

## Security

### The embedded server is a boundary

`127.0.0.1` is unreachable from the network and reachable by **every process on
the machine**. A packaged Rahti application listening there is a signed-in
session, an upload endpoint and a database, offered to all of them.

- The listener binds `127.0.0.1` and there is no API to bind anything else.
- The port is assigned by the operating system.
- `security.loopbackToken` (on by default) mints a token per launch. The shell
  opens `http://127.0.0.1:<port>/?__rahti_native=<token>`; the gate turns it
  into an `HttpOnly; SameSite=Strict` cookie and redirects to the clean URL.
  Everything after that carries the cookie because browsers carry cookies —
  pages, assets, `pp.rpc`, streaming responses, multipart uploads and the
  WebSocket handshake alike. Anything without it gets a bodyless 403.

  A cookie rather than a header is the whole design: a header can be attached
  to `fetch` and to nothing else, so a header scheme would allow rpcs and
  refuse the document.

  It is not authentication and does not replace CSRF. Rahti's own layers still
  answer "is there a session" and "did this call come from a page of ours".
- A release package sets `RAHTI_DEV=0` rather than inheriting it, so a
  developer with it exported cannot start a shipped application with its
  diagnostics endpoint and reload stream live.

### Content-Security-Policy

Served by the embedded server as a response header, because that is the only
policy a browser applies to a page on a `http://127.0.0.1` origin. `security.csp`
is the value; the copy in `tauri.conf.json` covers the shell's own placeholder
document.

The default permits no external source of anything — and permits
`'unsafe-eval'`, which is not an oversight. PulsePoint compiles the expressions
in a reactive block at runtime, building a render function with `new Function`;
that is what makes it a browser runtime rather than a build step. Remove the
directive and the server-rendered page still appears with every binding on it
dead, reporting an `EvalError` from inside the minified bundle. `script-src`
still refuses every source but this origin, so injected markup cannot load an
attacker's file.

### The native command bridge

In a browser, an XSS is a stolen session. In a native shell it is a stolen
session **and** whatever the page can reach through the bridge. So the bridge
is an allowlist, and it is five entries:

| Command | What it does |
| --- | --- |
| `platform` | Which operating system. |
| `app_version` | The installed version. |
| `app_data_dir` | Where this installation keeps its data, for showing the user. |
| `open_external` | Hands an `http`, `https` or `mailto` URL to the user's own browser. |
| `choose_file` | Opens the platform picker and returns what the user chose. |

Nothing runs a program. Nothing reads a path the page names — `choose_file`
returns what a *human* selected, and that difference is the security model.
`open_external` refuses `file:`, `javascript:` and `data:`: on Windows a
`file:` URL opens whatever the extension is associated with, including
executables.

A link to anywhere else opens in the user's browser and the window stays where
it is. A privileged WebView that navigated to an external page would be running
somebody else's HTML with the bridge attached.

Adding a command means three places, on purpose: a `#[tauri::command]` in
`native/src/lib.rs`, an entry in `rahti_native::commands()`, and a permission
in `native/capabilities/default.json`. Assume the page calling it has been
compromised, because it has to be safe in that case too.

## `window.rahtiNative` from a page

```rust
html! {
    <button onclick={openDocs()}>"Documentation"</button>
    <script>
        const native = window.rahtiNative;

        function openDocs() {
          const url = "https://example.com/docs";
          if (native && native.has("open-external")) {
            native.invoke("open_external", { url });
          } else {
            window.open(url, "_blank", "noopener");
          }
        }
    </script>
}
```

**The same page has to work as a website.** `window.rahtiNative` is undefined
in a browser, and a page that assumed otherwise is a page your web deployment
breaks on. Every use is a capability check with a web fallback.

### Why a global and not `pp.native`

Because a reactive block cannot see `pp.native`, and that was measured rather
than assumed.

PulsePoint compiles the expressions in a reactive block into a function of its
own making, and supplies the `pp` visible inside it. That `pp` is not
`window.pp`: `pp.state(...)` works there, and a property added to the global
`pp` is not visible. Attaching one and reading it back from a page returns
`undefined`.

Making `pp.native` work would mean changing the runtime bundle, which Rahti
does not do. `window` *is* reachable from inside a compiled block, so the
bridge is a global. The shell still attaches `pp.native` where it can, and
nothing is documented as depending on it.

The bridge is a framework-owned script the shell injects before any page script
runs. PulsePoint's bundle is not edited, patched or wrapped.

## Signing

Nothing about signing is in any committed file, and `native/.gitignore` refuses
keystores and certificates. Both platforms read it from the environment of the
build:

```text
RAHTI_NATIVE_WINDOWS_CERTIFICATE
RAHTI_NATIVE_WINDOWS_CERTIFICATE_PASSWORD

RAHTI_NATIVE_ANDROID_KEYSTORE
RAHTI_NATIVE_ANDROID_KEYSTORE_PASSWORD
RAHTI_NATIVE_ANDROID_KEY_ALIAS
RAHTI_NATIVE_ANDROID_KEY_PASSWORD
```

`doctor --release` says which are missing. `build --target android --debug`
produces a package that installs without any of them, which is what testing
wants.

The two platforms get there differently, and the Android one is worth knowing
about. Tauri reads the Windows certificate from the environment, so those two
variables are simply passed through. It reads **no** signing key from the
environment on Android — its documented flow is to add a `signingConfigs` block
to the generated Gradle file by hand — so `cargo rahti native` does that wiring
for you on every Android build, writing the key's details to
`native/gen/android/keystore.properties`. That file is inside the generated
directory the shell's `.gitignore` excludes, and is named there as well.

This matters because the failure is silent: passing a keystore and assuming
Tauri picks it up produces a release that builds fine and is **unsigned**,
which Google Play refuses and which will not install.

## Shutdown

The host owns it. Ctrl+C is not involved: a packaged GUI never receives one and
Android's lifecycle has nothing like it.

On exit the shell fires Rahti's shutdown broadcast — which ends the dev event
stream and every open socket, because a graceful shutdown would otherwise wait
for connections that were never going to close — then stops accepting and waits
a bounded five seconds for what is left. Dropping the server handle without a
shutdown also stops it, which is the Android case: the operating system can
destroy the process without giving anything a chance to run.

## What native packaging does not give you

Background services, exact alarms, home-screen widgets, biometrics, push
notifications, system tray, global shortcuts, registry access and Windows Hello
are **not** included and are not automatic. Each needs an optional Tauri plugin
or platform code, added to `native/` — which is yours — and each is a native
capability to be designed with the allowlist rules above in mind.
