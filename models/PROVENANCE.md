# Embedded model provenance

The ONNX files are downloaded unmodified. All inference is local and implemented
in Rust through the existing ONNX Runtime C ABI.

| File | Publisher / pinned source | SHA-256 | Published license metadata |
|---|---|---|---|
| anime-face.onnx | https://huggingface.co/deepghs/anime_face_detection/resolve/784dc4c0bb692351ddcdbe6131a050b17d3025d5/face_detect_v1.4_n/model.onnx | FD860B650A4377046842C3CD80D01B0B408BDFBDB4ACEE5759630F82C6EF04A9 | MIT |
| anime-eye.onnx | https://huggingface.co/deepghs/anime_eye_detection/resolve/ba69e3ee3b0b23e7c948994e182f06cd46e534f1/eye_detect_v1.0_n/model.onnx | A96515CDF4CA0A4CAF56778D93BA7FC28B4582727FF32645844CD89D39CDEE29 | OpenRAIL |

The exact upstream model cards are included alongside this file. The eye model
publisher provides an OpenRAIL metadata declaration in its model card rather
than a separate license text in that repository. The DeepGHS project MIT license
is included for attribution; it does not replace the eye model's declaration.
The model files are separate assets from Pyra's GPL-3.0 application code.

The face model produces face boxes, not eye landmarks. Pyra runs the eye model
on the same frame and associates eye detections with their containing face.
A single visible eye can anchor a profile's horizontal bar. YuNet remains the
fallback for real faces that have no specialist eye detection.
