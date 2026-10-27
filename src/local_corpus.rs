//! Opt-in local corpus diagnostics. Private inputs and crops never enter CI.
use crate::{
    detection::{Detector, Image, Region, suppress},
    tracking::Tracker,
};
use anyhow::Result;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires PYRA_PRIVATE_MANIFEST, PYRA_PRIVATE_REPORT and PYRA_RUNTIME; local inputs only"]
fn anime_scan_stability() -> Result<()> {
    let manifest = std::fs::read_to_string(std::env::var("PYRA_PRIVATE_MANIFEST")?)?;
    let output = PathBuf::from(std::env::var("PYRA_PRIVATE_REPORT")?);
    std::fs::create_dir_all(output.join("eyes"))?;
    let limit = std::env::var("PYRA_PRIVATE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);
    let mut detector = Detector::new(&PathBuf::from(std::env::var("PYRA_RUNTIME")?))?;
    let full = Region {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    let tiles = crate::scanning::grid(full.w, full.h, 800);
    let mut csv = String::from(
        "sample,reference_faces,drop_events,add_events,jump_events,max_center_jump_px\n",
    );
    let mut totals = [0usize; 5];
    let start = std::env::var("PYRA_PRIVATE_START")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut events = String::new();
    for (index, path) in manifest.lines().enumerate().skip(start).take(limit) {
        let Ok(source) = image::open(path) else {
            continue;
        };
        let im = source.thumbnail(1720, 900).to_rgb8();
        let mut canvas = image::RgbImage::from_pixel(1920, 1080, image::Rgb([235, 235, 235]));
        let x = (index * 137) % (1921 - im.width() as usize);
        let y = (index * 71) % (1081 - im.height() as usize);
        image::imageops::replace(&mut canvas, &im, x as i64, y as i64);
        let pixels: Vec<_> = canvas
            .pixels()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let frame = Image {
            pixels: &pixels,
            width: full.w,
            height: full.h,
        };
        let initial = detector.detect(&frame, full, 0.65)?;
        let mut reference = initial.clone();
        for region in &tiles {
            reference.extend(detector.detect(&frame, *region, 0.65)?);
        }
        let reference = suppress(reference);
        // Save only the upper face, ending below the eyes, for inspection.
        // Never emit a complete source image or its filename into the report.
        for (n, f) in reference.iter().enumerate() {
            if f.eye_band <= 0. || f.eye_count < 2 {
                continue;
            }
            let left = (f.eyes[0][0].min(f.eyes[1][0]) - f.w * 0.18).max(0.) as u32;
            let top = (f.eyes[0][1].min(f.eyes[1][1]) - f.h * 0.13).max(0.) as u32;
            let right = (f.eyes[0][0].max(f.eyes[1][0]) + f.w * 0.18)
                .min(1920.)
                .max(left as f32 + 1.) as u32;
            let bottom = (f.eyes[0][1].max(f.eyes[1][1]) + f.h * 0.13)
                .min(1080.)
                .max(top as f32 + 1.) as u32;
            if left < 1920 && top < 1080 && right > left && bottom > top {
                image::imageops::crop_imm(&canvas, left, top, right - left, bottom - top)
                    .to_image()
                    .save(output.join("eyes").join(format!("{index:03}-{n:02}.png")))?;
            }
        }
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.frame(&frame, now);
        tracker.update(initial, now);
        let (mut drops, mut adds, mut jumps, mut max_jump) = (0, 0, 0, 0f32);
        let scans = (tiles.len() + 2) * 4;
        for step in 0..scans {
            let time = now + Duration::from_millis(step as u64 * 150);
            tracker.frame(&frame, time);
            let region = match step % (tiles.len() + 2) {
                0 => full,
                1 if !tracker.tracks.is_empty() => crate::worker::face_region(
                    &tracker.tracks[(step / (tiles.len() + 2)) % tracker.tracks.len()].face,
                    full.w,
                    full.h,
                ),
                n => tiles[n.saturating_sub(2) % tiles.len()],
            };
            let before: Vec<_> = tracker.tracks.iter().map(|t| t.face.clone()).collect();
            let found = detector.detect(&frame, region, 0.65)?;
            let observations = found.clone();
            tracker.verify_at(
                &found,
                region,
                if step % (tiles.len() + 2) == 0 {
                    72.
                } else {
                    0.
                },
                time,
            );
            tracker.update(found, time);
            if step < (tiles.len() + 2) {
                continue;
            }
            for f in &before {
                if let Some(t) = tracker.tracks.iter().find(|t| t.face.same_face(f)) {
                    let (a, b) = (f.bar(1., 1.), t.face.bar(1., 1.));
                    let jump = (a.x - b.x).hypot(a.y - b.y);
                    max_jump = max_jump.max(jump);
                    jumps += usize::from(jump > 3.);
                    if jump > 3. {
                        events.push_str(&format!("sample={index} scan={step} region={region:?} JUMP {jump:.2}\nold={f:?}\nnew={:?}\n", t.face));
                    }
                } else {
                    drops += 1;
                    events.push_str(&format!("sample={index} scan={step} region={region:?} DROP\nold={f:?}\nobservations={observations:?}\n"));
                }
            }
            adds += tracker
                .tracks
                .iter()
                .filter(|t| before.iter().all(|f| !t.face.same_face(f)))
                .count();
        }
        totals[0] += 1;
        totals[1] += reference.len();
        totals[2] += drops;
        totals[3] += adds;
        totals[4] += jumps;
        csv.push_str(&format!(
            "{index},{},{drops},{adds},{jumps},{max_jump:.2}\n",
            reference.len()
        ));
        std::fs::write(output.join("stability.csv"), &csv)?;
        std::fs::write(output.join("events.txt"), &events)?;
        println!(
            "sample={index}; faces={}; drops={drops}; additions={adds}; jumps={jumps}; max_jump={max_jump:.2}px",
            reference.len()
        );
    }
    println!(
        "private static corpus totals: images={}, faces={}, drops={}, additions={}, jumps={}",
        totals[0], totals[1], totals[2], totals[3], totals[4]
    );
    Ok(())
}
