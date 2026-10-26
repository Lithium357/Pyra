# Third-party components

The application code is Rust; native inference is supplied by ONNX Runtime.

| Component | Purpose | License / upstream |
|---|---|---|
| YuNet 2023mar | Face detector and eye landmarks, embedded unmodified | MIT, Shiqi Yu; `models/LICENSE-YuNet.txt`; https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet |
| ONNX Runtime 1.22.0 | Native CPU inference via C ABI | MIT, Microsoft; https://github.com/microsoft/onnxruntime/tree/v1.22.0 |
| ort / ort-sys 2.0.0-rc.10 | Rust bindings to ONNX Runtime | MIT OR Apache-2.0; https://github.com/pykeio/ort |
| windows-sys / windows-link | Native Win32 FFI declarations | MIT OR Apache-2.0; https://github.com/microsoft/windows-rs |
| anyhow | Error propagation | MIT OR Apache-2.0; https://github.com/dtolnay/anyhow |
| libloading | Native runtime loading | ISC; https://github.com/nagisa/rust_libloading |
| ndarray and numeric support crates | ort's tensor infrastructure | MIT OR Apache-2.0; see Cargo.lock and individual crate manifests |
| smallvec | ort's bounded metadata storage | MIT OR Apache-2.0; https://github.com/servo/rust-smallvec |
| image and its codecs (development only) | Tests and manual scene fixture | MIT OR Apache-2.0; https://github.com/image-rs/image |

The original OpenCV YuNet decoder is a reference for the model's output layout;
the Rust preprocessing/decoding implementation is independent. The public OpenCV
test image is fetched only for optional local/CI tests and is not embedded or
distributed with the application.

When distributing binaries, include the ONNX Runtime release's LICENSE and
ThirdPartyNotices.txt alongside the Pyra/YuNet licenses. The portable package
also includes exact Rust dependency license texts exported from the Cargo sources.
