use crate::detection::{Face, Image, Region};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Gray {
    pixels: Vec<u8>,
    blurred: Vec<u8>,
    scratch: Vec<u8>,
    w: usize,
    h: usize,
}
impl Gray {
    fn fill(&mut self, image: &Image<'_>) {
        let scale = tracking_scale(image.width, image.height);
        self.w = (image.width as f32 * scale).round().max(1.) as usize;
        self.h = (image.height as f32 * scale).round().max(1.) as usize;
        self.pixels.resize(self.w * self.h, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                let at = ((y * image.height / self.h) * image.width + x * image.width / self.w) * 4;
                self.pixels[y * self.w + x] = ((image.pixels[at] as u16 * 29
                    + image.pixels[at + 1] as u16 * 150
                    + image.pixels[at + 2] as u16 * 77)
                    >> 8) as u8;
            }
        }
        self.blur();
    }
    fn blur(&mut self) {
        self.blurred.resize(self.w * self.h, 0);
        self.scratch.resize(self.w * self.h, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                let a = x.saturating_sub(2);
                let b = (x + 3).min(self.w);
                let sum: u32 = (a..b).map(|xx| self.pixels[y * self.w + xx] as u32).sum();
                self.scratch[y * self.w + x] = (sum / (b - a) as u32) as u8;
            }
        }
        for y in 0..self.h {
            for x in 0..self.w {
                let a = y.saturating_sub(2);
                let b = (y + 3).min(self.h);
                let sum: u32 = (a..b).map(|yy| self.scratch[yy * self.w + x] as u32).sum();
                self.blurred[y * self.w + x] = (sum / (b - a) as u32) as u8;
            }
        }
    }
    fn at(&self, x: i32, y: i32) -> Option<u8> {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            None
        } else {
            Some(self.pixels[y as usize * self.w + x as usize])
        }
    }
}

pub struct Track {
    pub face: Face,
    seen: Instant,
    detected: Instant,
    confirmed: Instant,
    missed_checks: u8,
}

fn tracking_scale(width: usize, height: usize) -> f32 {
    (480. / width.min(height) as f32)
        .min(2560. / width.max(height) as f32)
        .min(1.)
}

#[derive(Default)]
pub struct Tracker {
    pub tracks: Vec<Track>,
    previous: Gray,
    current: Gray,
}

impl Tracker {
    /// Track translation with sparse patch matching between full detections.
    /// Buffers and patch sample arrays are reused; no video history is retained.
    pub fn frame(&mut self, image: &Image<'_>, now: Instant) -> (bool, bool) {
        self.current.fill(image);
        let mut cut = self.current.w != self.previous.w || self.current.h != self.previous.h;
        let mut changed = cut;
        if !cut {
            let mut sum = 0u64;
            let mut changes = 0;
            for (&a, &b) in self.current.pixels.iter().zip(&self.previous.pixels) {
                let diff = a.abs_diff(b);
                sum += diff as u64;
                changes += usize::from(diff > 8);
            }
            changed = changes > 3;
            cut = sum as f64 / self.current.pixels.len().max(1) as f64 > 38.;
            if !cut {
                let sx = self.current.w as f32 / image.width as f32;
                let sy = self.current.h as f32 / image.height as f32;
                for track in &mut self.tracks {
                    if !changed {
                        track.seen = now;
                        track.detected = now;
                        continue;
                    }
                    if let Some((dx, dy)) =
                        displacement(&self.previous, &self.current, &track.face, sx, sy)
                    {
                        let mut candidate = track.face.clone();
                        candidate.translate(dx as f32 / sx, dy as f32 / sy);
                        let consistent = (dx == 0 && dy == 0)
                            || displacement(&self.current, &self.previous, &candidate, sx, sy)
                                .is_some_and(|(bx, by)| {
                                    ((dx + bx) as f32).hypot((dy + by) as f32) <= 3.
                                });
                        if consistent {
                            track.face = candidate;
                            track.seen = now;
                        }
                    }
                }
            }
        }
        if cut {
            self.tracks.clear();
        }
        self.tracks.retain(|t| {
            now.duration_since(t.seen) < Duration::from_millis(400)
                && now.duration_since(t.detected) < Duration::from_secs(30)
        });
        std::mem::swap(&mut self.previous, &mut self.current);
        (changed, cut)
    }

    pub fn update(&mut self, faces: Vec<Face>, now: Instant) {
        let old_count = self.tracks.len();
        let mut matched = vec![false; old_count];
        for mut face in faces {
            let best = self.tracks[..old_count]
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched[*i])
                .map(|(i, t)| (i, face.overlap(&t.face)))
                .filter(|(_, overlap)| *overlap > 0.22)
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = best {
                let old = &self.tracks[i].face;
                // A stable face box must not suddenly move its eye plane down
                // towards the mouth. Preserve normalized eye placement through
                // one inconsistent landmark result; actual box motion is kept.
                let expected = (old.eyes[0][1] + old.eyes[1][1]) * 0.5;
                let projected = face.y + (expected - old.y) / old.h.max(1.) * face.h;
                let measured = (face.eyes[0][1] + face.eyes[1][1]) * 0.5;
                if face.overlap(old) > 0.55
                    && (measured - projected).abs() > (face.h * 0.18).max(5.)
                {
                    for (eye, prior) in face.eyes.iter_mut().zip(old.eyes) {
                        eye[0] = face.x + (prior[0] - old.x) / old.w.max(1.) * face.w;
                        eye[1] = face.y + (prior[1] - old.y) / old.h.max(1.) * face.h;
                    }
                }
                let movement =
                    (face.eyes[0][0] - old.eyes[0][0]).hypot(face.eyes[0][1] - old.eyes[0][1]);
                if movement < face.w * 0.07 {
                    for (eye, prior) in face.eyes.iter_mut().zip(old.eyes) {
                        eye[0] = eye[0] * 0.8 + prior[0] * 0.2;
                        eye[1] = eye[1] * 0.8 + prior[1] * 0.2;
                    }
                }
                self.tracks[i] = Track {
                    face,
                    seen: now,
                    detected: now,
                    confirmed: now,
                    missed_checks: 0,
                };
                matched[i] = true;
            } else if self.tracks.len() < 512
                && self.tracks.iter().all(|t| t.face.overlap(&face) < 0.3)
            {
                self.tracks.push(Track {
                    face,
                    seen: now,
                    detected: now,
                    confirmed: now,
                    missed_checks: 0,
                });
            }
        }
    }

    /// A tracked patch can latch onto background after its face disappears.
    /// Two authoritative negative scans remove that stale bar even if the patch
    /// matcher keeps finding texture. Coarse scans don't invalidate tiny faces.
    pub fn verify(&mut self, detections: &[Face], region: Region, minimum_width: f32) {
        self.verify_at(detections, region, minimum_width, Instant::now());
    }
    fn verify_at(&mut self, detections: &[Face], region: Region, minimum_width: f32, now: Instant) {
        for track in &mut self.tracks {
            let f = &track.face;
            if f.w >= minimum_width
                && f.x >= region.x as f32
                && f.y >= region.y as f32
                && f.x + f.w <= (region.x + region.w) as f32
                && f.y + f.h <= (region.y + region.h) as f32
            {
                if detections.iter().any(|d| d.overlap(f) > 0.22) {
                    track.missed_checks = 0;
                    track.confirmed = now;
                } else {
                    track.missed_checks = track.missed_checks.saturating_add(1);
                }
            }
        }
        // Fast overlapping tiles can disagree within one frame. Require a short
        // confirmation grace period as well as negative checks to avoid flashing.
        self.tracks.retain(|t| {
            t.missed_checks < 2 || now.duration_since(t.confirmed) < Duration::from_millis(700)
        });
    }
}

fn displacement(previous: &Gray, current: &Gray, f: &Face, sx: f32, sy: f32) -> Option<(i32, i32)> {
    let mut samples = [(0i32, 0i32, 0u8, 0u8); 64];
    let mut n = 0;
    // Sample the central face area, keeping surrounding page text out of the patch.
    for y in 0..8 {
        for x in 0..8 {
            let px = ((f.x + f.w * (0.15 + x as f32 * 0.10)) * sx).round() as i32;
            let py = ((f.y + f.h * (0.15 + y as f32 * 0.10)) * sy).round() as i32;
            if let Some(v) = previous.at(px, py) {
                samples[n] = (
                    px,
                    py,
                    v,
                    previous.blurred[py as usize * previous.w + px as usize],
                );
                n += 1;
            }
        }
    }
    if n < 32 {
        return None;
    }
    let patch = &samples[..n];
    let mean = patch.iter().map(|s| s.2 as f32).sum::<f32>() / n as f32;
    if patch.iter().map(|s| (s.2 as f32 - mean).abs()).sum::<f32>() / (n as f32) < 5. {
        return None;
    }
    let cost = |dx, dy, blurred: bool| -> f32 {
        let mut total = 0u32;
        for &(x, y, raw, smooth) in patch {
            let Some(next) = current.at(x + dx, y + dy) else {
                return f32::INFINITY;
            };
            let next = if blurred {
                current.blurred[(y + dy) as usize * current.w + (x + dx) as usize]
            } else {
                next
            };
            let v = if blurred { smooth } else { raw };
            total += v.abs_diff(next) as u32;
        }
        total as f32 / n as f32 + ((dx * dx + dy * dy) as f32).sqrt() * 0.025
    };
    let mut best = (0, 0);
    let mut score = cost(0, 0, false);
    // Unchanged patches are common even when another part of the screen animates.
    if score < 2. {
        return Some(best);
    }
    score = cost(0, 0, true);
    for dy in (-32..=32).step_by(4) {
        for dx in (-32..=32).step_by(4) {
            let candidate = cost(dx, dy, true);
            if candidate < score {
                score = candidate;
                best = (dx, dy);
            }
        }
    }
    let coarse = best;
    score = cost(best.0, best.1, false);
    for dy in coarse.1 - 3..=coarse.1 + 3 {
        for dx in coarse.0 - 3..=coarse.0 + 3 {
            let candidate = cost(dx, dy, false);
            if candidate < score {
                score = candidate;
                best = (dx, dy);
            }
        }
    }
    if score < 28. { Some(best) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_eye_landmarks_do_not_jump_to_the_mouth_but_real_motion_is_kept() {
        let now = Instant::now();
        let good = Face {
            x: 20.,
            y: 20.,
            w: 100.,
            h: 100.,
            eyes: [[45., 60.], [95., 60.]],
            score: 0.95,
        };
        let mut tracker = Tracker::default();
        tracker.update(vec![good.clone()], now);
        let mut bad = good.clone();
        bad.eyes = [[45., 95.], [95., 95.]];
        bad.score = 0.7;
        tracker.update(vec![bad], now + Duration::from_millis(50));
        assert!((tracker.tracks[0].face.bar(1., 1.).y - 60.).abs() < 0.1);
        let mut moved = good;
        moved.translate(0., 12.);
        tracker.update(vec![moved], now + Duration::from_millis(100));
        assert!((tracker.tracks[0].face.bar(1., 1.).y - 72.).abs() < 0.1);
    }
    #[test]
    fn moving_sequence_has_no_track_dropouts_or_cumulative_vertical_drift() {
        let frame = |dx: usize, dy: usize| {
            let mut data = vec![230u8; 360 * 280 * 4];
            for y in 0..84 {
                for x in 0..64 {
                    let v = ((x * 71 + y * 37 + x * y * 13) % 230) as u8;
                    let at = ((40 + dy + y) * 360 + 70 + dx + x) * 4;
                    data[at..at + 3].fill(v);
                }
            }
            data
        };
        let now = Instant::now();
        let first = frame(0, 0);
        let mut tracker = Tracker::default();
        tracker.frame(
            &Image {
                pixels: &first,
                width: 360,
                height: 280,
            },
            now,
        );
        tracker.update(
            vec![Face {
                x: 70.,
                y: 40.,
                w: 64.,
                h: 84.,
                eyes: [[86., 68.], [118., 68.]],
                score: 0.95,
            }],
            now,
        );
        for step in 1usize..=120 {
            let p = step % 60;
            let triangle = if p <= 30 { p } else { 60 - p };
            let (dx, dy) = (triangle * 2, triangle * 3);
            let data = frame(dx, dy);
            tracker.frame(
                &Image {
                    pixels: &data,
                    width: 360,
                    height: 280,
                },
                now + Duration::from_millis(step as u64 * 33),
            );
            assert_eq!(tracker.tracks.len(), 1, "track flickered at frame {step}");
            let bar = tracker.tracks[0].face.bar(1., 1.);
            assert!(
                (bar.y - (68 + dy) as f32).abs() <= 1.,
                "vertical drift at frame {step}: {} vs {}",
                bar.y,
                68 + dy
            );
        }
    }
    #[test]
    fn wide_and_portrait_tracking_preserve_short_axis_detail() {
        for (w, h) in [(3440, 1440), (5120, 1440), (7680, 2160), (1440, 3440)] {
            let scale = tracking_scale(w, h);
            assert!((w.min(h) as f32 * scale - 480.).abs() < 1.);
            assert!(w.max(h) as f32 * scale <= 2560.);
        }
        assert_eq!(tracking_scale(400, 300), 1.);
    }
    #[test]
    fn patch_tracking_follows_translation() {
        let mut a = Gray {
            pixels: vec![0; 128 * 128],
            w: 128,
            h: 128,
            ..Gray::default()
        };
        let mut b = Gray {
            pixels: vec![0; 128 * 128],
            w: 128,
            h: 128,
            ..Gray::default()
        };
        for y in 20..90 {
            for x in 20..90 {
                let v = ((x * 71 + y * 37 + x * y * 13) % 256) as u8;
                a.pixels[y * 128 + x] = v;
                b.pixels[(y + 7) * 128 + x + 12] = v;
            }
        }
        a.blur();
        b.blur();
        let f = Face {
            x: 20.,
            y: 20.,
            w: 70.,
            h: 70.,
            eyes: [[40., 40.], [65., 40.]],
            score: 0.9,
        };
        assert_eq!(displacement(&a, &b, &f, 1., 1.), Some((12, 7)));
    }
    #[test]
    fn scene_cut_clears_tracks() {
        let mut t = Tracker::default();
        let dark = vec![0; 64 * 64 * 4];
        let light = vec![255; 64 * 64 * 4];
        t.frame(
            &Image {
                pixels: &dark,
                width: 64,
                height: 64,
            },
            Instant::now(),
        );
        t.update(
            vec![Face {
                x: 5.,
                y: 5.,
                w: 30.,
                h: 30.,
                eyes: [[10., 10.], [25., 10.]],
                score: 0.9,
            }],
            Instant::now(),
        );
        assert!(
            t.frame(
                &Image {
                    pixels: &light,
                    width: 64,
                    height: 64
                },
                Instant::now()
            )
            .1
        );
        assert!(t.tracks.is_empty());
    }
    #[test]
    fn negative_scans_remove_stale_large_faces_without_dropping_tiny_faces() {
        let mut tracker = Tracker::default();
        let large = Face {
            x: 20.,
            y: 20.,
            w: 100.,
            h: 120.,
            eyes: [[45., 60.], [95., 60.]],
            score: 0.9,
        };
        let mut tiny = large.clone();
        tiny.x = 200.;
        tiny.w = 25.;
        tiny.h = 30.;
        let now = Instant::now();
        tracker.update(vec![large, tiny], now);
        let region = Region {
            x: 0,
            y: 0,
            w: 1000,
            h: 800,
        };
        tracker.verify_at(&[], region, 50., now + Duration::from_millis(100));
        assert_eq!(tracker.tracks.len(), 2);
        tracker.verify_at(&[], region, 50., now + Duration::from_millis(800));
        assert_eq!(tracker.tracks.len(), 1);
        assert_eq!(tracker.tracks[0].face.w, 25.);
    }
}
