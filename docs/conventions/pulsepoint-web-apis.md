# Native Web APIs

A component's `<script>` is plain browser JavaScript, evaluated as a function
body in the page's own global scope. It is not a sandbox and no build step
rewrites it: `window`, `document`, and `navigator` are the objects every other
script on the page sees, and DevTools debug the script as written. Every web
API the browser ships is therefore available to a component directly:

- **WebGPU** (`navigator.gpu`) — GPU rendering and compute: large charts,
  simulations, image and video processing, in-browser ML inference;
- **Canvas 2D / WebGL** — drawing, charts, image processing;
- **Web Workers / OffscreenCanvas** — parsing, number crunching, or rendering
  off the main thread;
- **WebAssembly** — native-speed modules compiled from Rust, C, or Go;
- **Web Audio**, **media capture**, and **WebRTC** — synthesis, camera,
  microphone, screen share, calls;
- **IndexedDB / Cache Storage** and the **Intersection / Resize / Mutation
  observers**;
- **Web Serial / WebUSB / WebHID / Web Bluetooth**, plus Clipboard,
  Notifications, File System Access, and Geolocation.

PulsePoint wraps none of them, and none needs a wrapper: do not invent
`pp.gpu`, `pp.worker`, or similar. When a task needs heavy or hardware-backed
work, write the browser API inside the component that owns it. Do not declare
the task out of reach, add a second frontend framework, or push per-frame work
through `pp.state`.

The parts divide the work:

| Job | Use |
| --- | --- |
| Data from the server | `#[rpc]` + `pp.rpc`, `RpcStream`, `#[socket]` + `pp.socket`, or `@{Json(&value)}` for the first render |
| Values the markup shows | `pp.state` — a change re-renders only the nodes that differ |
| Handles to browser objects | `pp.ref` — devices, buffers, contexts, workers, audio graphs, streams; a ref change never re-renders |
| Acquire, feed, release | `pp.effect` — create on mount, push new state in when it changes, dispose in the cleanup |
| The heavy work | the browser API itself — shaders, threads, codecs, and hardware run at native speed, outside PulsePoint |

A WebGPU chart fed by an rpc. The server renders the first series into the
page, and each click asks for a new one:

```rust
use crate::rahti::{Html, Json, html, rpc};

fn series(points: u32, phase: f32) -> Vec<f32> {
    (0..points.clamp(8, 512))
        .map(|i| 0.5 + 0.4 * (phase + i as f32 / 6.0).sin())
        .collect()
}

#[rpc]
pub async fn gpu_series(points: u32, phase: f32) -> Vec<f32> {
    series(points, phase)
}

pub async fn page() -> Html {
    let initial = series(48, 0.0);
    html! {
        <section>
            <canvas pp-ref={canvas} width="640" height="240"></canvas>
            <button onclick={load()}>"Refresh"</button>
            <p hidden={gpuSupported}>"WebGPU is unavailable; drawing with Canvas 2D."</p>
            <script>r##"
                const canvas = pp.ref(null);
                const gpu = pp.ref(null);                // device, pipeline, buffers: never state
                const [series, setSeries] = pp.state(@{Json(&initial)});
                const [ready, setReady] = pp.state(false);
                const gpuSupported = !!navigator.gpu;

                // Acquire the GPU once; release it on unmount.
                pp.effect(() => {
                    if (!navigator.gpu) { setReady(true); return; }
                    let cancelled = false;
                    initGpu(canvas.current).then((g) => {
                        if (cancelled) return g?.device.destroy();
                        gpu.current = g;
                        setReady(true);
                    });
                    return () => {
                        cancelled = true;
                        gpu.current?.device.destroy();
                        gpu.current = null;
                    };
                }, []);

                // Every new series: write it to a GPU buffer and draw.
                pp.effect(() => {
                    if (!ready) return;
                    gpu.current ? drawGpu(gpu.current, series) : draw2d(canvas.current, series);
                }, [series, ready]);

                async function load() {
                    const values = await pp.rpc("gpu_series", { points: 64, phase: Math.random() * 6.28 });
                    setSeries(values);
                }

                // initGpu, drawGpu, and draw2d are plain WebGPU and Canvas code:
                // navigator.gpu.requestAdapter(), adapter.requestDevice(),
                // canvas.getContext("webgpu"), a render pipeline,
                // device.queue.writeBuffer(...), a render pass.
            "##</script>
        </section>
    }
}
```

A continuous animation belongs to the browser's frame loop, not to the render
cycle; state only starts and stops it:

```javascript
const frame = pp.ref(0);
const [running, setRunning] = pp.state(true);

pp.effect(() => {
    if (!running) return;
    let id = requestAnimationFrame(function tick(t) {
        frame.current += 1;             // a ref: no render per frame
        renderFrame(gpu.current, t);    // the GPU does the per-frame work
        id = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(id);
}, [running]);
```

A worker takes data from the server and hands its answer back to state:

```javascript
const worker = pp.ref(null);
const [result, setResult] = pp.state(null);

pp.effect(() => {
    const w = new Worker("/js/parse-worker.js", { type: "module" });
    w.onmessage = (event) => setResult(event.data);
    worker.current = w;
    return () => w.terminate();
}, []);

async function analyze() {
    worker.current.postMessage(await pp.rpc("export_rows"));
}
```

## Third-Party Libraries

Any library that runs in a browser runs in a component. Vendor it under
`public/js/`, load it with `import()` inside an effect, give it the element
from a ref, and destroy it in the cleanup:

```javascript
pp.effect(() => {
    let cancelled = false;
    let chart = null;
    import('/js/vendor/chart-lib.js').then(({ Chart }) => {
        if (cancelled) return;
        chart = new Chart(host.current, { data: points });
    });
    return () => { cancelled = true; chart?.destroy(); };
}, []);
```

## DOM Ownership for Games, Three.js and Widgets

Give an integration a dedicated element through `pp-ref`, and decide one
owner for every attribute and every subtree:

- **A library-sized canvas:** `<canvas pp-ref={canvas}></canvas>`. When the
  renderer sets the drawing-buffer `width` and `height`, leave them out of
  the template: a re-render removes only attributes the previous render
  wrote, so the renderer's values survive. An attribute the template renders
  stays the template's — the next render may overwrite it — so a library
  cannot take over a bound `width`, `class` or `style` by changing it. Put
  template layout styles on a wrapper.
- **Library-built DOM:** `<div pp-ref={host}></div>`. An element with a ref
  and **no authored children** keeps the children a library inserts — a
  terminal, an editor, a map, a chart — across every re-render. Keep the host
  literally empty: no text, no whitespace children, no bindings, no
  `pp-for`, no nested component. Put loading and error text beside it.
- **First-party UI** stays PulsePoint markup beside the host: controls,
  labels, lists and forms with `on*` handlers and `pp.state`. Never scan or
  rebuild it imperatively.

Preservation lasts while the element is retained. Removing it, changing its
key, or navigating away disposes the component, so destroy the widget, stop
frame loops, disconnect observers and release GPU resources in the cleanup.

A callback the integration keeps — a frame tick, a library's `onChange`, a
socket's `onMessage` — is wrapped in `pp.event(…)` so it reads current state
and props (see `pulsepoint-hooks.md`). Positions, frame times and engine
handles live in refs; only labels and meaningful UI changes go in state.

### A Three.js Lifecycle

The vendored module sits at `public/js/vendor/three/three.module.js`. The
template renders an empty canvas inside a sized wrapper and a Pause button
beside it; the renderer owns the drawing-buffer size:

```rust
html! {
    <section class="relative h-96">
        <canvas pp-ref={canvas} class="block h-full w-full"></canvas>
        <button onclick={setPaused(!paused)}>{paused ? "Resume" : "Pause"}</button>
        <p hidden={!error}>{error}</p>
        <script>r##"
            const canvas = pp.ref(null);
            const engine = pp.ref(null);
            const [paused, setPaused] = pp.state(false);
            const [error, setError] = pp.state('');

            const draw = pp.event((time) => {
                const e = engine.current;
                if (!e) return;
                if (!paused) e.mesh.rotation.y = time / 1000;
                e.renderer.render(e.scene, e.camera);
            });

            pp.effect(() => {
                let cancelled = false;
                let frame = 0;
                let observer = null;
                let dispose = () => {};
                (async () => {
                    try {
                        const THREE = await import('/js/vendor/three/three.module.js');
                        if (cancelled) return;
                        const renderer = new THREE.WebGLRenderer({ canvas: canvas.current });
                        const scene = new THREE.Scene();
                        const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 100);
                        camera.position.z = 3;
                        const geometry = new THREE.BoxGeometry();
                        const material = new THREE.MeshNormalMaterial();
                        const mesh = new THREE.Mesh(geometry, material);
                        scene.add(mesh);
                        dispose = () => { material.dispose(); geometry.dispose(); renderer.dispose(); };
                        engine.current = { renderer, scene, camera, mesh };
                        observer = new ResizeObserver(([entry]) => {
                            const { width, height } = entry.contentRect;
                            if (!width || !height) return;
                            renderer.setSize(width, height, false); // the renderer owns width/height
                            camera.aspect = width / height;
                            camera.updateProjectionMatrix();
                        });
                        observer.observe(canvas.current.parentElement);
                        frame = requestAnimationFrame(function tick(time) {
                            draw(time);
                            frame = requestAnimationFrame(tick);
                        });
                    } catch (failure) {
                        if (!cancelled) setError(failure.message ?? 'The renderer is unavailable');
                    }
                })();
                return () => {
                    cancelled = true;
                    cancelAnimationFrame(frame);
                    observer?.disconnect();
                    dispose();
                    engine.current = null;
                };
            }, [draw]);
        "##</script>
    </section>
}
```

The wrapper has a defined size, the canvas fills it through classes, and the
renderer sets only the drawing buffer. Dispose of textures, controls and
every other resource the scene owns in the same cleanup.

## Verifying an Integration

A browser test (`rahti::browser`, see "Browser Tests" in `testing.md`) proves
the ownership contract where a person would notice it breaking:

- change visible state after the integration starts, then check the canvas
  size, the library's attributes and the host's children survived;
- check template-owned `hidden` and `disabled` attributes still update;
- pause and resume read current state without recreating the engine;
- navigate away and back: the old frame loop stops, resources are released,
  and the new mount initializes once;
- end with `browser.errors()` empty.

## Rules

- Browser objects — GPU devices and buffers, contexts, workers, audio
  contexts, media streams, peer connections, observers, database handles —
  live in `pp.ref`, never in `pp.state`.
- Acquire in an effect with `[]` dependencies and release in its cleanup:
  `device.destroy()`, `worker.terminate()`, `audioContext.close()`,
  `track.stop()`, `peerConnection.close()`, `observer.disconnect()`,
  `db.close()`. SPA navigation unmounts the component, so a missing cleanup
  leaks a GPU device or a running worker into the next page.
- Cleanups are synchronous. Start async setup (`requestAdapter()`,
  `getUserMedia()`, `WebAssembly.instantiateStreaming()`) inside the effect
  and guard it with a `cancelled` flag, as above.
- Feed the API from a second effect whose dependencies are the state it reads.
  That effect is the bridge from server data to the GPU, worker, or audio
  graph.
- Per-frame values live in refs and per-frame work runs on
  `requestAnimationFrame`. Never call a state setter every frame; set state
  only when something on screen changes.
- Feature-detect (`if (!navigator.gpu)`) and keep a fallback. WebGPU and most
  device APIs need a secure context: HTTPS, or `localhost` in development.
- APIs that need a user gesture — the hardware `request*()` pickers,
  clipboard writes, fullscreen, the first `AudioContext.resume()` — run inside
  the `on…` handler that received it, not in an effect.
- A component script has no static `import`/`export` and no top-level
  `await`. Load a library with `import()` inside an effect, or with its own
  `<script type="module">` in a root layout.
- Worker scripts, `.wasm` modules, shaders, and model files go in `public/`,
  which serves them at the URL root.
- Rahti's default CSP admits this: `worker-src 'self' blob:`, `script-src
  'self' 'unsafe-eval'` (WebAssembly compilation needs `'unsafe-eval'`), and
  `connect-src` with `https:` and `blob:`. A worker script, a library loaded
  with `import()`, and a `.wasm` module must be same-origin (or a `blob:` URL
  for a worker): vendor them into `public/`. An application that narrows its policy must keep these
  sources for the APIs it uses; see `security-middleware.md`.
- Results that must stay authoritative or secret — prices, permissions,
  anything persisted — are computed in Rust behind `#[rpc]`. The browser API
  is for rendering, interaction, and client-side acceleration.
