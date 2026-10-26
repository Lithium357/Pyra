use crate::detection::{Face, Image, Region, suppress};
use anyhow::{Result, ensure};
use ort::{session::Session, value::TensorRef};
#[cfg(test)]
use std::path::Path;

pub struct Stylized {
    face: Session,
    eye: Session,
    input: Vec<f32>,
    side: usize,
}

fn session(bytes: &[u8]) -> Result<Session> {
    Ok(Session::builder()?
        .with_intra_threads(2)?
        .with_inter_threads(1)?
        .with_parallel_execution(false)?
        .with_config_entry("session.intra_op.allow_spinning", "0")?
        .with_config_entry("session.inter_op.allow_spinning", "0")?
        .commit_from_memory(bytes)?)
}

impl Stylized {
    pub fn set_mode(&mut self, mode: u32) {
        self.side = match mode {
            1 => 640,
            2 => 384,
            _ => 512,
        };
    }
    pub fn new() -> Result<Self> {
        Ok(Self {
            face: session(include_bytes!("../models/anime-face.onnx"))?,
            eye: session(include_bytes!("../models/anime-eye.onnx"))?,
            input: vec![0.; 3 * 512 * 512],
            side: 512,
        })
    }

    #[cfg(test)]
    pub fn from_directory(path: &Path) -> Result<Self> {
        let face = session(&std::fs::read(path.join("anime-face.onnx"))?)?;
        let eye = session(&std::fs::read(path.join("anime-eye.onnx"))?)?;
        let side = std::env::var("PYRA_ART_SIDE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(512);
        ensure!(
            [256, 320, 384, 512, 640].contains(&side),
            "invalid anime inference size"
        );
        Ok(Self {
            face,
            eye,
            input: vec![0.; 3 * side * side],
            side,
        })
    }

    pub fn detect(
        &mut self,
        image: &Image<'_>,
        region: Region,
        threshold: f32,
    ) -> Result<Vec<Face>> {
        ensure!(
            region.w > 0
                && region.h > 0
                && region.x + region.w <= image.width
                && region.y + region.h <= image.height
                && image.pixels.len() == image.width * image.height * 4,
            "invalid anime capture region"
        );
        let side = if region.w.max(region.h) <= 384 {
            self.side.min(256)
        } else {
            self.side
        };
        self.input.resize(3 * side * side, 0.);
        // Model reference uses RGB [0,1] stretched to its input rectangle.
        for y in 0..side {
            let py = ((y as f32 + 0.5) * region.h as f32 / side as f32 - 0.5)
                .clamp(0., (region.h - 1) as f32);
            let y0 = py as usize;
            let y1 = (y0 + 1).min(region.h - 1);
            let fy = py - y0 as f32;
            for x in 0..side {
                let px = ((x as f32 + 0.5) * region.w as f32 / side as f32 - 0.5)
                    .clamp(0., (region.w - 1) as f32);
                let x0 = px as usize;
                let x1 = (x0 + 1).min(region.w - 1);
                let fx = px - x0 as f32;
                for c in 0..3 {
                    let at = |xx, yy| {
                        image.pixels[((yy + region.y) * image.width + xx + region.x) * 4 + 2 - c]
                            as f32
                    };
                    self.input[c * side * side + y * side + x] =
                        ((at(x0, y0) * (1. - fx) + at(x1, y0) * fx) * (1. - fy)
                            + (at(x0, y1) * (1. - fx) + at(x1, y1) * fx) * fy)
                            / 255.;
                }
            }
        }
        let boxes = run(&mut self.face, &self.input, side, region, threshold)?;
        if boxes.is_empty() {
            return Ok(Vec::new());
        }
        let eyes = run(&mut self.eye, &self.input, side, region, 0.25)?;
        let mut result = Vec::new();
        for mut face in boxes {
            let mut contained: Vec<_> = eyes
                .iter()
                .filter(|e| {
                    let (x, y) = (e.x + e.w * 0.5, e.y + e.h * 0.5);
                    x >= face.x
                        && x <= face.x + face.w
                        && y >= face.y
                        && y <= face.y + face.h * 0.85
                })
                .collect();
            contained.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
            if !contained.is_empty() {
                let a = contained[0];
                let b = *contained.get(1).unwrap_or(&a);
                face.eye_count = contained.len().min(2) as u8;
                // Illustrated eyes occupy more of the face than human eyes.
                // Keep their height plus a modest localization margin covered.
                face.eye_band = (a.h.max(b.h) * 1.4).max(face.h * 0.34);
                face.eyes = [
                    [a.x + a.w * 0.5, a.y + a.h * 0.5],
                    [b.x + b.w * 0.5, b.y + b.h * 0.5],
                ];
                result.push(face);
            }
        }
        Ok(suppress(result))
    }
}

fn run(
    session: &mut Session,
    input: &[f32],
    side: usize,
    region: Region,
    threshold: f32,
) -> Result<Vec<Face>> {
    let tensor = TensorRef::from_array_view(([1usize, 3, side, side], input))?;
    let outputs = session.run(ort::inputs![tensor])?;
    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    ensure!(
        shape.len() == 3 && shape[1] == 5,
        "unexpected anime model output {shape:?}"
    );
    let n = shape[2] as usize;
    let sx = region.w as f32 / side as f32;
    let sy = region.h as f32 / side as f32;
    let mut found = Vec::new();
    for i in 0..n {
        let score = data[4 * n + i];
        if score < threshold {
            continue;
        }
        let w = data[2 * n + i] * sx;
        let h = data[3 * n + i] * sy;
        let x = data[i] * sx - w * 0.5 + region.x as f32;
        let y = data[n + i] * sy - h * 0.5 + region.y as f32;
        if [x, y, w, h, score].iter().all(|v| v.is_finite()) && w >= 3. && h >= 3. {
            found.push(Face {
                x,
                y,
                w,
                h,
                eyes: [[x + w * 0.3, y + h * 0.4], [x + w * 0.7, y + h * 0.4]],
                score,
                eye_count: 0,
                detail: w / sx,
                eye_band: 0.,
            });
        }
    }
    Ok(suppress(found))
}
