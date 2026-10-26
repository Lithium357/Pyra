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

Neither these short runs nor CI establish months of uptime or correctness on
every monitor/DPI/GPU combination. Legacy Lenna tests remain for geometry and
capture regressions; they are not the main evidence of detection quality.
