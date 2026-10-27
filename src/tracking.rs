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
            let py = ((y as f32 + 0.5) * image.height as f32 / self.h as f32 - 0.5)
                .clamp(0., (image.height - 1) as f32);
            let y0 = py as usize;
            let y1 = (y0 + 1).min(image.height - 1);
            let fy = py - y0 as f32;
            for x in 0..self.w {
                let px = ((x as f32 + 0.5) * image.width as f32 / self.w as f32 - 0.5)
                    .clamp(0., (image.width - 1) as f32);
                let x0 = px as usize;
                let x1 = (x0 + 1).min(image.width - 1);
                let fx = px - x0 as f32;
                let gray = |xx, yy| {
                    let at = (yy * image.width + xx) * 4;
                    (image.pixels[at] as f32 * 29.
                        + image.pixels[at + 1] as f32 * 150.
                        + image.pixels[at + 2] as f32 * 77.)
                        / 256.
                };
                let a = gray(x0, y0) * (1. - fx) + gray(x1, y0) * fx;
                let b = gray(x0, y1) * (1. - fx) + gray(x1, y1) * fx;
                self.pixels[y * self.w + x] = (a * (1. - fy) + b * fy).round() as u8;
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

    fn sample(&self, x: f32, y: f32) -> Option<f32> {
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        let a = self.at(ix, iy)? as f32 * (1. - fx) + self.at(ix + 1, iy)? as f32 * fx;
        let b = self.at(ix, iy + 1)? as f32 * (1. - fx) + self.at(ix + 1, iy + 1)? as f32 * fx;
        Some(a * (1. - fy) + b * fy)
    }
}

pub struct Track {
    pub face: Face,
    seen: Instant,
    detected: Instant,
    confirmed: Instant,
    missed_checks: u8,
    weak_eyes: u8,
    appearance_changed: bool,
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
    /// Move asynchronous detections from their source frame into the latest
    /// captured frame. Reject scene cuts or motion the patch matcher cannot
    /// explain rather than drawing bars at obsolete coordinates.
    pub fn project(&mut self, image: &Image<'_>, faces: Vec<Face>) -> Option<Vec<Face>> {
        self.current.fill(image);
        if self.current.w != self.previous.w || self.current.h != self.previous.h {
            return None;
        }
        let change: u64 = self
            .current
            .pixels
            .iter()
            .zip(&self.previous.pixels)
            .map(|(&a, &b)| a.abs_diff(b) as u64)
            .sum();
        if change as f64 / self.current.pixels.len().max(1) as f64 > 38. {
            return None;
        }
        let sx = self.current.w as f32 / image.width as f32;
        let sy = self.current.h as f32 / image.height as f32;
        let mut projected = Vec::with_capacity(faces.len());
        for mut face in faces {
            if let Some((dx, dy)) = displacement(&self.current, &self.previous, &face, sx, sy) {
                let original = face.clone();
                face.translate(dx / sx, dy / sy);
                if displacement(&self.previous, &self.current, &face, sx, sy)
                    .is_some_and(|(bx, by)| (dx + bx).hypot(dy + by) <= 3.)
                {
                    projected.push(face);
                } else if dx == 0. && dy == 0. {
                    projected.push(original);
                }
            }
        }
        Some(projected)
    }
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
                    // Accumulate changes until a detector result consumes them.
                    // Other windows or a desktop clock must not make a static
                    // face's repeated, slightly different landmarks jitter.
                    track.appearance_changed |=
                        patch_changed(&self.previous, &self.current, &track.face, sx, sy);
                    if let Some((dx, dy)) =
                        displacement(&self.previous, &self.current, &track.face, sx, sy)
                    {
                        let mut candidate = track.face.clone();
                        candidate.translate(dx / sx, dy / sy);
                        let consistent = (dx == 0. && dy == 0.)
                            || displacement(&self.current, &self.previous, &candidate, sx, sy)
                                .is_some_and(|(bx, by)| (dx + bx).hypot(dy + by) <= 3.);
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
                .filter(|(i, overlap)| *overlap > 0.22 || self.tracks[*i].face.same_face(&face))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = best {
                let old = &self.tracks[i].face;
                // A low-resolution overview is evidence that the face remains,
                // but must not replace landmarks refined by a close local scan.
                // Keep tracking its current frame position until a comparably
                // detailed scan can establish a real pose/scale change.
                if face.same_face(old)
                    && (face.detail < old.detail * 0.6
                        || (!self.tracks[i].appearance_changed
                            && !self.previous.pixels.is_empty()
                            && face.eye_count == old.eye_count
                            && face.detail <= old.detail * 1.15))
                {
                    let track = &mut self.tracks[i];
                    track.seen = now;
                    track.detected = now;
                    track.confirmed = now;
                    track.missed_checks = 0;
                    matched[i] = true;
                    continue;
                }
                let weak_eyes = if face.eye_count < old.eye_count {
                    self.tracks[i].weak_eyes.saturating_add(1)
                } else {
                    0
                };
                // A stable face box must not suddenly move its eye plane down
                // towards the mouth. Preserve normalized eye placement through
                // one inconsistent landmark result; actual box motion is kept.
                let expected = (old.eyes[0][1] + old.eyes[1][1]) * 0.5;
                let projected = face.y + (expected - old.y) / old.h.max(1.) * face.h;
                let measured = (face.eyes[0][1] + face.eyes[1][1]) * 0.5;
                if face.overlap(old) > 0.55
                    && ((face.eye_count < old.eye_count && weak_eyes <= 2)
                        || (face.eye_count == old.eye_count
                            && face.detail <= old.detail * 1.5
                            && (measured - projected).abs() > (face.h * 0.18).max(5.)))
                {
                    for (eye, prior) in face.eyes.iter_mut().zip(old.eyes) {
                        eye[0] = face.x + (prior[0] - old.x) / old.w.max(1.) * face.w;
                        eye[1] = face.y + (prior[1] - old.y) / old.h.max(1.) * face.h;
                    }
                    face.eye_count = old.eye_count;
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
                    weak_eyes,
                    appearance_changed: false,
                };
                matched[i] = true;
            } else if self.tracks.len() < 512
                && self.tracks.iter().all(|t| !t.face.same_face(&face))
            {
                self.tracks.push(Track {
                    face,
                    seen: now,
                    detected: now,
                    confirmed: now,
                    missed_checks: 0,
                    weak_eyes: 0,
                    appearance_changed: false,
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
                if detections
                    .iter()
                    .any(|d| d.overlap(f) > 0.22 || d.same_face(f))
                {
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

fn patch_changed(previous: &Gray, current: &Gray, f: &Face, sx: f32, sy: f32) -> bool {
    let mut changed = 0;
    for y in 0..8 {
        for x in 0..8 {
            let px = ((f.x + f.w * (0.1 + x as f32 * 0.11)) * sx).round() as i32;
            let py = ((f.y + f.h * (0.1 + y as f32 * 0.11)) * sy).round() as i32;
            if let (Some(a), Some(b)) = (previous.at(px, py), current.at(px, py)) {
                changed += usize::from(a.abs_diff(b) > 2);
            }
        }
    }
    changed > 1
}

fn displacement(previous: &Gray, current: &Gray, f: &Face, sx: f32, sy: f32) -> Option<(f32, f32)> {
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
    if score < 0.5 {
        return Some((0., 0.));
    }
    if patch.iter().map(|s| (s.2 as f32 - mean).abs()).sum::<f32>() / (n as f32) < 5. {
        return None;
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
    // Integer steps on a 480-pixel tracking image can mean several physical
    // screen pixels. Quarter-pixel refinement prevents those rounded steps
    // accumulating into drift between landmark refreshes.
    let fractional_cost = |dx: f32, dy: f32| -> f32 {
        let mut sum = 0.;
        for &(x, y, raw, _) in patch {
            let Some(value) = current.sample(x as f32 + dx, y as f32 + dy) else {
                return f32::INFINITY;
            };
            sum += (raw as f32 - value).abs();
        }
        sum / n as f32 + dx.hypot(dy) * 0.025
    };
    let mut refined = (best.0 as f32, best.1 as f32);
    score = fractional_cost(refined.0, refined.1);
    for y in -3..=3 {
        for x in -3..=3 {
            let dx = best.0 as f32 + x as f32 * 0.25;
            let dy = best.1 as f32 + y as f32 * 0.25;
            let candidate = fractional_cost(dx, dy);
            if candidate < score {
                score = candidate;
                refined = (dx, dy);
            }
        }
    }
    if score < 28. { Some(refined) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_face_stays_still_while_other_screen_content_changes() {
        let now = Instant::now();
        let mut pixels = vec![128; 200 * 200 * 4];
        let mut tracker = Tracker::default();
        tracker.frame(
            &Image {
                pixels: &pixels,
                width: 200,
                height: 200,
            },
            now,
        );
        let face = Face {
            x: 40.,
            y: 40.,
            w: 100.,
            h: 100.,
            eyes: [[65., 70.], [115., 80.]],
            score: 0.9,
            eye_count: 2,
            detail: 100.,
            eye_band: 0.,
        };
        tracker.update(vec![face.clone()], now);
        pixels[..20 * 4].fill(0);
        tracker.frame(
            &Image {
                pixels: &pixels,
                width: 200,
                height: 200,
            },
            now + Duration::from_millis(50),
        );
        let mut noisy = face.clone();
        noisy.translate(2., 3.);
        tracker.update(vec![noisy], now + Duration::from_millis(60));
        assert_eq!(tracker.tracks[0].face.eyes, face.eyes);
        // A pose change inside the face must release the stationary guard.
        for y in 40..140 {
            for x in 40..140 {
                pixels[(y * 200 + x) * 4..(y * 200 + x) * 4 + 3].fill(140);
            }
        }
        tracker.frame(
            &Image {
                pixels: &pixels,
                width: 200,
                height: 200,
            },
            now + Duration::from_millis(100),
        );
        let mut pose = face.clone();
        pose.eyes[1][1] += 8.;
        tracker.update(vec![pose], now + Duration::from_millis(110));
        assert!(tracker.tracks[0].face.eyes[1][1] > face.eyes[1][1] + 5.);
    }
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
            eye_count: 2,
            detail: 100.,
            eye_band: 0.,
        };
        let mut tracker = Tracker::default();
        tracker.update(vec![good.clone()], now);
        let mut bad = good.clone();
        bad.eyes = [[45., 95.], [95., 95.]];
        bad.score = 0.7;
        tracker.update(vec![bad], now + Duration::from_millis(50));
        assert!((tracker.tracks[0].face.bar(1., 1.).y - 60.).abs() < 0.1);
        let mut moved = good.clone();
        moved.translate(0., 12.);
        tracker.update(vec![moved], now + Duration::from_millis(100));
        assert!((tracker.tracks[0].face.bar(1., 1.).y - 72.).abs() < 0.1);
        let mut tracker = Tracker::default();
        let mut coarse = good.clone();
        coarse.eye_count = 1;
        coarse.eyes = [[75., 95.], [75., 95.]];
        tracker.update(vec![coarse], now);
        tracker.update(vec![good.clone()], now + Duration::from_millis(100));
        assert!(
            (tracker.tracks[0].face.bar(1., 1.).y - 60.).abs() < 0.1,
            "two-eye refinement was frozen at the coarse eye plane"
        );
        let mut profile = good;
        profile.eye_count = 1;
        profile.eyes = [[75., 65.], [75., 65.]];
        for n in 1..=3 {
            tracker.update(
                vec![profile.clone()],
                now + Duration::from_millis(100 + n * 100),
            );
        }
        assert_eq!(
            tracker.tracks[0].face.eye_count, 1,
            "sustained profile must eventually replace old frontal eyes"
        );
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
                eye_count: 2,
                detail: 100.,
                eye_band: 0.,
            }],
            now,
        );
        let original = tracker.tracks[0].face.clone();
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
            if step == 5 {
                let projected = tracker
                    .project(
                        &Image {
                            pixels: &first,
                            width: 360,
                            height: 280,
                        },
                        vec![original.clone()],
                    )
                    .expect("same scene");
                assert_eq!(projected.len(), 1, "delayed detection was lost");
                assert!(
                    (projected[0].eyes[0][1] - (68 + dy) as f32).abs() <= 1.,
                    "delayed detection retained stale coordinates"
                );
            }
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
            eye_count: 2,
            detail: 100.,
            eye_band: 0.,
        };
        assert_eq!(displacement(&a, &b, &f, 1., 1.), Some((12., 7.)));
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
                eye_count: 2,
                detail: 100.,
                eye_band: 0.,
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
            eye_count: 2,
            detail: 100.,
            eye_band: 0.,
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
