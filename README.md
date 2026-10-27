# Pyra

Automatic live **eye bars** on faces across your Windows desktop. Run it once;
it detects faces, tracks their motion, and draws tilted black bars through the
eyes without clicking individual faces. The application is written in Rust.
There is no Python runtime, browser shell, web service, or network code.

Real faces use YuNet; anime and illustrated faces also use dedicated face and
eye models. Bars follow detected eyes, including a single visible eye in a
profile, rather than assuming the eyes occupy a fixed part of a face box.
Faces cut off by an internal scan tile are left to overlapping tiles and overview
scans, preventing a large wallpaper face from becoming several partial tracks.
Repeated scans preserve bar geometry on unchanged face pixels; higher-resolution
refinements and changes inside the face can still update the eye placement.

Every bar displays **[censored]** in white Doto text, centered and rotated with
the bar. The font is embedded and loaded privately; no font installation or
network access is needed. A reusable prefiltered label bitmap scales to fit
each bar without changing its eye coverage. Very small bars have tiny labels.

## Run

Windows 10 version 2004 (build 19041) or newer, or Windows 11; x64.

1. Extract the entire portable package into a writable folder.
2. Run `Pyra.exe`. Keep `onnxruntime.dll` and
   `onnxruntime_providers_shared.dll` beside it.
3. Click the lavender/black-bar icon in the Windows notification area for settings.
   Windows may put it under the notification area's overflow arrow.

**Ctrl+Alt+B** pauses/resumes. **Ctrl+Alt+O** opens the tray menu at your cursor.
If another app has registered those shortcuts, the tray menu still works.
Choose **Quit Pyra** to stop it. Launching a second instance does nothing.

The tray menu offers:

| Setting | Behavior |
|---|---|
| Balanced | Overlapping 800-pixel local regions, supplementary overview scans, and high-resolution checks of tracked faces; target 20 capture/tracking cycles/s. |
| More detail | 640-pixel local regions and 640-pixel anime inference; target 25 capture/tracking cycles/s. |
| Low power | 1280-pixel regions and 384-pixel anime inference; target 10 capture/tracking cycles/s. Small faces are more likely to be missed. |
| Sensitivity | Lower model confidence catches more faces, with more false positives. |
| Bar width/thickness | Adjusts only the eye bar; never switches to full-face masks. |
| Start when I sign in | Opt-in current-user startup entry. Disable before moving/deleting the portable folder. |

These are scheduling targets, not guaranteed frame rates. Multiple monitors,
large windows, and busy screens increase cycle time. Per-region fingerprints
keep changed regions pending until scanned, even if content immediately becomes
still. Inference runs on a separate thread, with one scan in flight and no frame
backlog. Monitors and overview/local/face scans take turns. Detection positions
are projected forward to the latest frame; obsolete results and scene cuts are
discarded. Subpixel motion avoids cumulative rounding drift. Close-up landmarks
survive coarse scans; duplicate boxes at different scales share one track.
Stable regions are verified infrequently and unchanged overlay uploads
are skipped. Balanced mode uses 512-pixel anime inference; small face checks use
256 pixels. The models use up to two CPU inference threads without idle spinning.

Ultrawide and portrait displays use local inference regions instead of depending
on a whole display shrunk to 640 pixels. Tracking targets 480 pixels on the short
axis, bounded at 2560 pixels on the long axis. Monitor positions remain signed
physical pixel coordinates, including displays above/left of the primary display.
Mirrored monitor rectangles are deduplicated. Overlay geometry is reasserted to
its physical rectangle after scaling or layout changes.

## Privacy and limits

- All screen processing is local. Frames live in memory and are overwritten;
  no screenshots, facial identities, or video are saved. Diagnostic logs contain
  failures, startup/recovery events, and monitor dimensions, not images.
- Bars are ordinary, capturable overlay windows: **desktop screenshots and
  desktop recordings include them**. Pyra's detector privately replaces each
  affected window's visible region from one coherent underlying window frame,
  preserving foreground occlusion and looking through transparent click-through
  utility overlays. Hardware-accelerated browsers and common Qt/WPF applications use a
  cached window-render fallback. Caches are shared across monitors and bounded
  to four reusable window bitmaps. No overlay is globally hidden from capture.
- A capture of a specific application window can omit external overlay windows,
  including Pyra. Use desktop/display capture when the bars must be included.
  This remains a best-effort live filter, not guaranteed irreversible redaction.
- New faces can appear before a bar is placed. Small faces, profiles, obscured
  eyes, drawings, and fast video cuts can be missed. Tracking handles translation;
  fresh landmark detections update tilt, scale, and facial movement. Overlapping
  scans are combined once per frame; bidirectional motion checks and eye-plane
  consistency checks reject ambiguous jumps.
- Protected video, secure desktops/UAC, and some exclusive fullscreen apps cannot
  be reliably captured or covered. Use windowed/borderless mode where possible.
- All active monitors are enumerated automatically, using physical pixel
  coordinates. Refreshing topology handles monitor additions/removals, scaling,
  and resolution changes. A face cut in half by a monitor seam can be missed.
- Detection/tracking is bounded at 512 simultaneous faces per monitor. No pending
  frame queue or growing video history is retained.

## Recovery and long uptime

The small native tray process supervises a separate capture/detection/overlay
process through atomic shared memory. If that process exits or stops advancing
its heartbeat for 15 seconds, the supervisor kills its Windows Job Object and
starts a fresh worker. Repeated failures use up to 30 seconds of backoff. Killing
or quitting the supervisor also kills its worker, preventing orphan overlays.

Capture/detection failures rebuild the worker. A short responsiveness check
precedes accelerated window rendering; an app or driver that subsequently stalls
is still covered by the worker watchdog. Display changes and sleep/resume
recreate display resources. The supervisor reasserts overlay z-order every second
and the worker is proactively recycled every 12 hours to bound long-term native
allocator drift. Recovery/recycling briefly removes the bars. The tray reports
live/idle, paused, or recovering rather than silently claiming normal operation.
Explorer restarts recreate the tray icon.

Settings and bounded rotating logs are in `data/` beside the executable. Logs
rotate at approximately 1 MB, with two backups per process type. Settings are
replaced atomically. Configuration is portable; sign-in startup uses the current
executable's absolute path.

The recovery design supports unattended operation, but **months of uptime have
not been validated**. Run extended soak tests on your own monitor/GPU/workload
before treating it as proven for that duration.

## Build from source

Install the stable Rust MSVC toolchain and Visual Studio C++ Build Tools/Windows
SDK. The Rust code calls native Windows APIs directly. Inference uses the native
ONNX Runtime C API through `ort`; there is no OpenCV runtime dependency.

```powershell
cargo build --release --locked
```

Download Microsoft's official
[ONNX Runtime 1.22.0 Windows x64 package](https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-x64-1.22.0.zip).
The ZIP's SHA-256 is
`174c616efc0271194488642a72f1a514e01487da4dfe84c49296d66e40ebe0da`.
Copy the two DLLs from its `lib/` folder beside `target/release/pyra.exe`.
The three models are embedded from `models/`; see `models/PROVENANCE.md` for the
anime models' exact versions, checksums, and published license metadata.
Some systems may need Microsoft's Visual C++ 2015–2022 x64 Redistributable.

Runtime loading uses an absolute DLL path beside the executable. For development
only, `PYRA_RUNTIME` can select another ONNX Runtime DLL. The application never
downloads a model or dependency at runtime.

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The primary image-quality regression corpus covers anime/game art, profiles,
glasses, multiple characters, no-face controls, dim/blurred/tilted variants, and
nine annotated real group photos. CI fetches checksum-pinned fixtures and runs
the Rust evaluation and real-time asynchronous motion tests. See
[validation/README.md](validation/README.md) for metrics, sources and limitations.

The legacy desktop/model tests need an unlocked Windows desktop, the runtime
DLL, and the OpenCV reference `lena.jpg` (not distributed with this repo):

```powershell
$env:PYRA_RUNTIME = 'C:\absolute\path\onnxruntime.dll'
$env:PYRA_TEST_IMAGE = 'C:\absolute\path\lena.jpg'
cargo test --release --locked --bin pyra -- --include-ignored --skip browser_window_surface --nocapture --test-threads=1
```

Those tests check real-model eye coordinates, mixed-size faces, real-model
5120×1440/1080×3840 canvases, complete scan grids up to 11520-pixel-wide layouts,
pending scan persistence/fairness, rotated bars, NMS, tracking, scene cuts,
negative physical coordinates, ultrawide/portrait overlay pixels, screenshot
visibility, private detector restoration, click-through flags, and GDI resource
cleanup across 25 overlay lifecycles. These synthetic/off-screen geometry tests
do not substitute for testing every physical monitor/DPI/GPU combination.
The motion regressions measure eye position over 90 real-face frames and 120
textured translation frames. A desktop timing regression changes the source
window between capture and restoration to reject mixed-frame eye-strip patches.
The image crate is a development
dependency used only by tests/the optional scene fixture.

For a native manual scene with four face sizes:

```powershell
cargo run --release --example test_scene -- C:\absolute\path\lena.jpg
```

Space toggles motion; Escape closes the scene. Test while other applications
animate/scroll, then test sleep/resume, display changes, and multi-day idle/video
workloads. Kill the worker PID from `data/pyra.log` to exercise crash recovery;
the supervisor should remain alive and start a new worker automatically.

An additional ignored `browser_window_surface_captures_all_four_faces` regression
expects a local Chrome window titled `Pyra browser coverage test - Google Chrome`
showing four copies of the reference image (400, 260, 160, 128 pixels wide).
It verifies that native accelerated rendering plus local scans retain all four
faces, then runs 60 capture/tracking frames with visible bars and requires each
eye position to stay within 4 pixels of its unoccluded reference. Close other
Pyra instances and keep the fixture visible when running it explicitly.

`Pyra.exe --status` writes current health to standard output when invoked with
redirected output. `--diagnostics` loads the model, runs blank-frame inference,
enumerates displays, and writes the result to `data/pyra.log`. `--quit` asks the
running tray instance to exit cleanly. `--background` suppresses the startup tip.

## Architecture

| Module | Responsibility |
|---|---|
| `native.rs` | Win32 tray/menu/hotkeys, settings, sign-in startup, shared state, Job Object and watchdog. |
| `capture.rs` | Monitor enumeration, reusable BGRA capture, visible-region restoration beneath bars, and bounded accelerated window-render caches; RAII cleanup. |
| `detection.rs` | Reusable bilinear BGR input tensor, YuNet inference/landmark decoding, NMS, detailed tiles, eye-bar geometry. |
| `stylized.rs` | Native anime face/eye inference, RGB preprocessing, eye association and single-eye profiles. |
| `analysis.rs` | Bounded inference thread; one job/result and no growing queue. |
| `tracking.rs` | Sparse coarse/fine patch tracking with blurred coarse search, short missed-match persistence and scene-cut resets. |
| `overlay.rs` | Reusable ARGB surfaces and capturable, transparent, topmost, click-through native windows. |
| `scanning.rs` | Aspect-independent local grids, persistent per-region fingerprints, and fair pending scan selection. |
| `worker.rs` | Fair asynchronous scan scheduling, source-frame projection, static-content suppression, topology/resource recovery and overlay updates. |

GDI capture favors a small dependency footprint and straightforward recovery. It
still copies screen pixels, and restoring hardware-accelerated windows adds work.
High-refresh 4K multi-monitor use would benefit from a future asynchronous GPU
window-capture backend and GPU composition. Switching to
Rust removes interpreter/UI-framework overhead; it does not make model inference
or desktop copies free.

## License

Pyra uses the repository's GPL-3.0 license. YuNet's model is MIT-licensed; see
`models/LICENSE-YuNet.txt`. ONNX Runtime and its native third-party notices must
accompany distributed DLLs. See `THIRD_PARTY.md` for upstream attribution.
