use anyhow::{Context, Result, ensure};
use ort::{session::Session, value::TensorRef};
use std::path::Path;

pub const INPUT: usize = 640;

#[derive(Clone, Debug)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub eyes: [[f32; 2]; 2],
    pub score: f32,
}

impl Face {
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
        for eye in &mut self.eyes {
            eye[0] += dx;
            eye[1] += dy;
        }
    }

    pub fn overlap(&self, other: &Self) -> f32 {
        let w = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let h = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        let intersection = w.max(0.) * h.max(0.);
        intersection / (self.w * self.h + other.w * other.h - intersection).max(1.)
    }

    pub fn bar(&self, width: f32, thickness: f32) -> Bar {
        let [mut a, mut b] = self.eyes;
        if a[0] > b[0] {
            std::mem::swap(&mut a, &mut b);
        }
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let distance = dx.hypot(dy);
        let mut angle = dy.atan2(dx);
        let mut x = (a[0] + b[0]) * 0.5;
        if distance < self.w * 0.12 || angle.abs() > 1.14 {
            x = self.x + self.w * 0.5;
            angle = 0.;
        }
        Bar {
            x,
            y: (a[1] + b[1]) * 0.5,
            w: (self.w * 1.08).max(distance * 2.15).max(12.) * width,
            h: (self.h * 0.24).max(distance * 0.52).max(7.) * thickness,
            angle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub angle: f32,
}

pub struct Image<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

pub struct Detector {
    session: Session,
    input: Vec<f32>,
}

impl Detector {
    pub fn new(runtime: &Path) -> Result<Self> {
        ort::init_from(runtime.display().to_string())
            .with_name("Pyra")
            .commit()?;
        let session = Session::builder()?
            .with_intra_threads(2)?
            .with_inter_threads(1)?
            .with_parallel_execution(false)?
            // Avoid idle spin threads, especially on static pages and while paused.
            .with_config_entry("session.intra_op.allow_spinning", "0")?
            .with_config_entry("session.inter_op.allow_spinning", "0")?
            .commit_from_memory(include_bytes!("../models/yunet.onnx"))
            .context("load bundled YuNet model")?;
        Ok(Self {
            session,
            input: vec![0.; 3 * INPUT * INPUT],
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
                && region.y + region.h <= image.height,
            "invalid capture region"
        );
        ensure!(
            image.pixels.len() == image.width * image.height * 4,
            "invalid BGRA frame"
        );
        self.input.fill(0.);
        let scale = INPUT as f32 / region.w.max(region.h) as f32;
        let rw = (region.w as f32 * scale).round().max(1.) as usize;
        let rh = (region.h as f32 * scale).round().max(1.) as usize;
        // Bilinear BGR, planar floats, original 0..255 range. Letterbox padding
        // occupies the right and bottom; landmark coordinates use the same scale.
        for y in 0..rh {
            let sy = ((y as f32 + 0.5) / scale - 0.5).clamp(0., (region.h - 1) as f32);
            let y0 = sy as usize;
            let y1 = (y0 + 1).min(region.h - 1);
            let fy = sy - y0 as f32;
            for x in 0..rw {
                let sx = ((x as f32 + 0.5) / scale - 0.5).clamp(0., (region.w - 1) as f32);
                let x0 = sx as usize;
                let x1 = (x0 + 1).min(region.w - 1);
                let fx = sx - x0 as f32;
                let at = |xx, yy, c| {
                    image.pixels[((yy + region.y) * image.width + xx + region.x) * 4 + c] as f32
                };
                for c in 0..3 {
                    let top = at(x0, y0, c) * (1. - fx) + at(x1, y0, c) * fx;
                    let bottom = at(x0, y1, c) * (1. - fx) + at(x1, y1, c) * fx;
                    self.input[c * INPUT * INPUT + y * INPUT + x] = top * (1. - fy) + bottom * fy;
                }
            }
        }
        let tensor =
            TensorRef::from_array_view(([1usize, 3, INPUT, INPUT], self.input.as_slice()))?;
        let outputs = self.session.run(ort::inputs![tensor])?;
        let mut faces = Vec::new();
        for stride in [8usize, 16, 32] {
            let cols = INPUT / stride;
            let (_, cls) = outputs[format!("cls_{stride}")].try_extract_tensor::<f32>()?;
            let (_, obj) = outputs[format!("obj_{stride}")].try_extract_tensor::<f32>()?;
            let (_, bbox) = outputs[format!("bbox_{stride}")].try_extract_tensor::<f32>()?;
            let (_, kps) = outputs[format!("kps_{stride}")].try_extract_tensor::<f32>()?;
            ensure!(
                cls.len() == cols * cols
                    && obj.len() == cls.len()
                    && bbox.len() == cls.len() * 4
                    && kps.len() == cls.len() * 10,
                "unexpected model output dimensions"
            );
            let unit = stride as f32 / scale;
            for i in 0..cls.len() {
                let score = (cls[i].clamp(0., 1.) * obj[i].clamp(0., 1.)).sqrt();
                if score < threshold {
                    continue;
                }
                let col = (i % cols) as f32;
                let row = (i / cols) as f32;
                let w = bbox[i * 4 + 2].exp() * unit;
                let h = bbox[i * 4 + 3].exp() * unit;
                let x = (col + bbox[i * 4]) * unit - w * 0.5 + region.x as f32;
                let y = (row + bbox[i * 4 + 1]) * unit - h * 0.5 + region.y as f32;
                let eyes = std::array::from_fn(|n| {
                    [
                        (col + kps[i * 10 + n * 2]) * unit + region.x as f32,
                        (row + kps[i * 10 + n * 2 + 1]) * unit + region.y as f32,
                    ]
                });
                if [
                    x, y, w, h, eyes[0][0], eyes[0][1], eyes[1][0], eyes[1][1], score,
                ]
                .iter()
                .all(|v| v.is_finite())
                    && w >= 9.
                    && h >= 9.
                    && x + w * 0.5 < (region.x + region.w) as f32
                    && y + h * 0.5 < (region.y + region.h) as f32
                {
                    faces.push(Face {
                        x,
                        y,
                        w,
                        h,
                        eyes,
                        score,
                    });
                }
            }
        }
        Ok(suppress(faces))
    }
}

pub fn suppress(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for face in faces {
        if kept.iter().all(|f| f.overlap(&face) < 0.3) {
            kept.push(face);
        }
        if kept.len() >= 512 {
            break;
        }
    }
    kept
}

pub fn tiles(width: usize, height: usize) -> Vec<Region> {
    let positions = |length: usize| {
        let count = length.saturating_sub(768).div_ceil(600);
        (0..=count)
            .map(|i| {
                (i * length.saturating_sub(768))
                    .checked_div(count)
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    for y in positions(height) {
        for x in positions(width) {
            out.push(Region {
                x,
                y,
                w: 768.min(width - x),
                h: 768.min(height - y),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bars_cover_both_eyes_and_follow_tilt() {
        let f = Face {
            x: 10.,
            y: 20.,
            w: 100.,
            h: 120.,
            eyes: [[35., 50.], [80., 65.]],
            score: 0.9,
        };
        let b = f.bar(1., 1.);
        assert!(b.angle > 0.3 && b.angle < 0.4);
        let (s, c) = b.angle.sin_cos();
        for eye in f.eyes {
            let dx = eye[0] - b.x;
            let dy = eye[1] - b.y;
            assert!((dx * c + dy * s).abs() < b.w / 2.);
            assert!((-dx * s + dy * c).abs() < b.h / 2.);
        }
    }
    #[test]
    fn tiles_cover_odd_and_negative_origin_independent_displays() {
        for (w, h) in [(1920, 1080), (3840, 2160), (500, 700), (1, 1), (1200, 1920)] {
            let t = tiles(w, h);
            for y in (0..h).step_by(13) {
                for x in (0..w).step_by(13) {
                    assert!(
                        t.iter()
                            .any(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
                    );
                }
            }
        }
    }
    #[test]
    fn nms_retains_distinct_faces() {
        let f = Face {
            x: 0.,
            y: 0.,
            w: 100.,
            h: 100.,
            eyes: [[25., 30.], [75., 30.]],
            score: 0.9,
        };
        let mut duplicate = f.clone();
        duplicate.x = 3.;
        let mut second = f.clone();
        second.x = 150.;
        assert_eq!(suppress(vec![f, duplicate, second]).len(), 2);
    }
    #[test]
    #[ignore = "requires PYRA_RUNTIME and PYRA_TEST_IMAGE; see README"]
    fn actual_model_multi_face_and_small_face_tiles() -> Result<()> {
        let runtime = std::env::var("PYRA_RUNTIME")?;
        let source = image::open(std::env::var("PYRA_TEST_IMAGE")?)?.to_rgba8();
        let mut detector = Detector::new(Path::new(&runtime))?;
        let bgra = |im: &image::RgbaImage| {
            im.pixels()
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect::<Vec<_>>()
        };
        let pixels = bgra(&source);
        let image = Image {
            pixels: &pixels,
            width: source.width() as usize,
            height: source.height() as usize,
        };
        let region = Region {
            x: 0,
            y: 0,
            w: image.width,
            h: image.height,
        };
        let faces = detector.detect(&image, region, 0.65)?;
        ensure!(
            faces.len() == 1,
            "expected one reference face, got {}",
            faces.len()
        );
        let bar = faces[0].bar(1., 1.);
        ensure!(
            (bar.x - 300.).abs() < 15. && (bar.y - 273.).abs() < 15.,
            "incorrect eye landmarks {bar:?}"
        );
        let mut canvas =
            image::RgbaImage::from_pixel(1600, 1000, image::Rgba([240, 240, 240, 255]));
        for (x, y, size) in [
            (0, 0, 512),
            (650, 20, 320),
            (1000, 500, 160),
            (100, 700, 128),
        ] {
            let small =
                image::imageops::resize(&source, size, size, image::imageops::FilterType::Triangle);
            image::imageops::overlay(&mut canvas, &small, x, y);
        }
        let pixels = bgra(&canvas);
        let frame = Image {
            pixels: &pixels,
            width: 1600,
            height: 1000,
        };
        let mut all = detector.detect(
            &frame,
            Region {
                x: 0,
                y: 0,
                w: 1600,
                h: 1000,
            },
            0.65,
        )?;
        for tile in tiles(1600, 1000) {
            all.extend(detector.detect(&frame, tile, 0.65)?);
        }
        let all = suppress(all);
        ensure!(
            all.len() == 4,
            "expected 4 mixed-size faces, got {}",
            all.len()
        );
        let start = std::time::Instant::now();
        for _ in 0..100 {
            detector.detect(&image, region, 0.65)?;
        }
        println!(
            "actual model: 1 reference + 4 mixed-size faces; warm mean {:.2} ms",
            start.elapsed().as_secs_f64() * 10.
        );
        Ok(())
    }
}
