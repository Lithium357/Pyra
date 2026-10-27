# Detection and tracking regression corpus

`sources.tsv` pins 21 public image fixtures by repository revision and SHA-256:
12 anime/game-art/control fixtures (including resized/cropped versions of the
same character mosaic), and nine real group photographs. The images are fetched
only for testing; they are not embedded in Pyra or redistributed in its portable
application. The manifest and tests do not require Python.

The anime mosaic contains 39 visible faces. `anime-eyes.txt` contains approximate
eye centers marked manually on its 1536-pixel reference copy, independently of
the detectors being evaluated. A one-visible-eye profile repeats that point.
Matching assigns each predicted face to at most one reference face. Coverage
means both marked eye centers lie inside the actual rotated bar geometry;
it does not mean the complete eye outlines are guaranteed covered.

The nine real photographs contain 43 annotated faces. `annotations.tsv` uses
the existing dlib example boxes and mean eye-contour points from its 68-point
landmark annotations. Those landmarks are model estimates, not new human labels.

These are regression fixtures, not an independently held-out accuracy benchmark.
Some fixtures come from upstream detector test suites. Crops, resizes, blur,
lighting changes and repeated motion frames are reported as variants, not as
additional independent source images. Real 3D game footage and arbitrary drawing
styles still need further validation.

## Run

The Windows workflow downloads every manifest URL, verifies its SHA-256, and
runs all `validation::` tests. It uploads `Pyra-validation` with CSV metrics,
per-image overlays and a contact sheet, alongside the portable build artifact.

To run locally, download the manifest files into a directory preserving their
`anime/` and `dlib/` paths. Set these environment variables to absolute paths:

```powershell
$env:PYRA_RUNTIME = 'C:\path\onnxruntime.dll'
$env:PYRA_CORPUS = 'C:\path\corpus\anime'
$env:PYRA_PHOTOS = 'C:\path\corpus\dlib'
$env:PYRA_ART_MODELS = (Resolve-Path models).Path
$env:PYRA_REPORT = 'C:\path\report'
cargo test --release --locked --bin pyra validation:: -- --ignored --nocapture --test-threads=1
```

- `varied_image_benchmark` compares YuNet-only with the combined detector and
  measures manually marked eye coverage at several scales, under dim lighting,
  blur and ±12° tilt. It includes two no-face controls and retains extra boxes
  and misses in the results. Its per-case regression floors retain room for
  numerical differences; the CSV contains the measured results.
- `annotated_real_groups` checks that the illustrated-face specialist does not
  reduce real-photo eye coverage compared with YuNet-only.
- `varied_anime_motion` moves three different characters, including glasses and
  a profile, through 90 frames with periodic fresh detection.
- `asynchronous_anime_motion` runs the actual bounded inference thread against
  120 moving 1920×1080 frames at a 50 ms cadence, alternating overview, tile and
  close-up scans. After 40 warmup frames it requires at least 98% eye-center
  coverage and limits duplicate bars to 1% of measured face-frames. It records
  scan count and frame processing time.
- `asynchronous_fast_scrolling` moves the same three characters by up to 240
  pixels between 1920×1080 captures, with reversals and sudden stops. Results
  are deliberately delivered only every third capture to test delayed detection
  projection. It checks 240 face-frames after warmup using the same coverage and
  duplicate limits. Set `PYRA_SCROLL_DESKTOP=1` on an unlocked desktop with Pyra
  stopped to additionally exercise real native overlays and window capture.
  Every captured source pixel must match the newly rendered image, including
  regions covered by the previous bar positions. The fixture must fit on the
  first display. This is a scrolling simulation, not an automated browser test.
- `tracking::tests::dense_page_scrolls_preserve_tracks_and_project_delayed_detections`
  separately checks large pixel changes over a textured page with a fixed header,
  so scrolling does not clear tracks as though it were an unrelated scene cut.
- `static_image_scan_consistency` repeatedly alternates overview, tile and face
  scans of one enlarged illustrated face. Internal scan edges must not create
  extra one-eyed tracks or move the stationary bar. By default it uses a public
  corpus crop. Set `PYRA_STATIC_IMAGE` to a local image to reproduce a private
  wallpaper problem; the image is neither saved to reports nor uploaded. Set
  `PYRA_STATIC_DESKTOP=1` on an unlocked desktop to render that image in a temporary
  native window and exercise actual overlay capture for 90 cycles. The fixture
  must fit on the first display. Every captured source pixel must remain identical
  to the image despite the visible overlay. This does not change desktop settings.

`capture::tests::desktop_surface_excludes_overlay` separately checks Explorer's
actual Progman/WorkerW surface while a temporary bar is visible. Run it explicitly
with `--ignored --nocapture` on a stationary, unlocked desktop with Pyra stopped.

Neither these short runs nor CI establish months of uptime or correctness on
every monitor/DPI/GPU combination. Legacy Lenna tests remain for geometry and
capture regressions; they are not the main evidence of detection quality.

## Private local collections

`private_validation::anime_scan_stability` is an opt-in local diagnostic, outside
the CI validation group. Set `PYRA_PRIVATE_MANIFEST` to a UTF-8 file with one local
image path per line, `PYRA_PRIVATE_REPORT` to a local output directory, and
`PYRA_RUNTIME` to the runtime DLL. `PYRA_PRIVATE_LIMIT` defaults to 128 images;
`PYRA_PRIVATE_START` selects a later portion of the manifest.

The diagnostic fits each still image into a 1920×1080 scene and repeatedly scans
overview, overlapping tiles and face regions without changing any source pixels.
It logs track removals, additions and bar-center jumps over 3 pixels after initial
acquisition. One-time improvements from more detailed landmarks count as jumps
too; these metrics measure temporal behavior, not ground-truth detection accuracy.
False positives can be among the reported candidates. Only narrow eye-area crops
of illustrated detections are exported for inspection, with numeric filenames.
It never exports complete input images or source paths to the report.

Inputs, manifests, crops and reports must stay outside the repository and CI
artifacts. The portable application contains none of this test data or test code.
