# Pyra

Automatic live **eye bars** on faces across your Windows desktop. Run it once;
it detects faces, tracks their motion, and draws tilted black bars through the
eyes without clicking individual faces. The application is written in Rust.
There is no Python runtime, browser shell, web service, or network code.

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
| Balanced | 640-pixel whole-display inference, plus overlapping detailed tiles; target 20 capture/tracking cycles/s when changing. |
| More detail | More frequent detections and tile scans; target 25 cycles/s. |
| Low power | Less frequent whole-display detection, no detailed tile scans; target 10 cycles/s. Small faces are more likely to be missed. |
| Sensitivity | Lower model confidence catches more faces, with more false positives. |
| Bar width/thickness | Adjusts only the eye bar; never switches to full-face masks. |
| Start when I sign in | Opt-in current-user startup entry. Disable before moving/deleting the portable folder. |

These are scheduling targets, not guaranteed frame rates. Multiple monitors and
busy screens increase cycle time. Static content is sampled at 10 Hz; repeated
inference and unchanged overlay uploads are skipped. Initial detailed tile scans
continue even on static content, so small stationary faces can be discovered.

## Privacy and limits

- All screen processing is local. Frames live in memory and are overwritten;
  no screenshots, facial identities, or video are saved. Diagnostic logs contain
  failures, startup/recovery events, and monitor dimensions, not images.
- This is a **viewing overlay**, not a redacted recording pipeline. The bars
  deliberately exclude themselves from standard capture APIs so the detector can
  keep seeing the underlying eyes. Screen recordings/screenshots may therefore
  show the original faces.
- New faces can appear before a bar is placed. Small faces, profiles, obscured
  eyes, drawings, and fast video cuts can be missed. Tracking handles translation;
  fresh landmark detections update tilt, scale, and facial movement.
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

Capture/detection failures rebuild the worker. Display changes and sleep/resume
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
The model is embedded in the executable from `models/yunet.onnx`.
Some systems may need Microsoft's Visual C++ 2015–2022 x64 Redistributable.

Runtime loading uses an absolute DLL path beside the executable. For development
only, `PYRA_RUNTIME` can select another ONNX Runtime DLL. The application never
downloads a model or dependency at runtime.

```powershell
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The optional desktop/model tests need an unlocked Windows desktop, the runtime
DLL, and the OpenCV reference `lena.jpg` (not distributed with this repo):

```powershell
$env:PYRA_RUNTIME = 'C:\absolute\path\onnxruntime.dll'
$env:PYRA_TEST_IMAGE = 'C:\absolute\path\lena.jpg'
cargo test --release --locked -- --include-ignored --nocapture --test-threads=1
```

Those tests check real-model eye coordinates, four mixed-size faces using the
detailed tiles, rotated bar geometry, NMS, tiled coverage, motion tracking, scene
cuts, physical overlay geometry, capture exclusion, click-through flags, and GDI
resource cleanup across 25 overlay lifecycles. The image crate is a development
dependency used only by tests/the optional scene fixture.

For a native manual scene with four face sizes:

```powershell
cargo run --release --example test_scene -- C:\absolute\path\lena.jpg
```

Space toggles motion; Escape closes the scene. Test while other applications
animate/scroll, then test sleep/resume, display changes, and multi-day idle/video
workloads. Kill the worker PID from `data/pyra.log` to exercise crash recovery;
the supervisor should remain alive and start a new worker automatically.

`Pyra.exe --status` writes current health to standard output when invoked with
redirected output. `--diagnostics` loads the model, runs blank-frame inference,
enumerates displays, and writes the result to `data/pyra.log`. `--quit` asks the
running tray instance to exit cleanly. `--background` suppresses the startup tip.

## Architecture

| Module | Responsibility |
|---|---|
| `native.rs` | Win32 tray/menu/hotkeys, settings, sign-in startup, shared state, Job Object and watchdog. |
| `capture.rs` | Monitor enumeration and reusable top-down BGRA GDI capture surfaces; RAII resource cleanup. |
| `detection.rs` | Reusable bilinear BGR input tensor, YuNet inference/landmark decoding, NMS, detailed tiles, eye-bar geometry. |
| `tracking.rs` | Sparse coarse/fine patch tracking with blurred coarse search, short missed-match persistence and scene-cut resets. |
| `overlay.rs` | Reusable ARGB surfaces and transparent, topmost, click-through, capture-excluded native windows. |
| `worker.rs` | Adaptive scan scheduling, static-content inference suppression, topology/resource recovery and overlay updates. |

GDI capture favors a small dependency footprint and straightforward recovery. It
still copies screen pixels; high-refresh 4K multi-monitor use would benefit from a
future DXGI/Desktop Duplication capture backend and GPU composition. Switching to
Rust removes interpreter/UI-framework overhead; it does not make model inference
or desktop copies free.

## License

Pyra uses the repository's GPL-3.0 license. YuNet's model is MIT-licensed; see
`models/LICENSE-YuNet.txt`. ONNX Runtime and its native third-party notices must
accompany distributed DLLs. See `THIRD_PARTY.md` for upstream attribution.
