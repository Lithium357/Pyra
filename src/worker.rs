use crate::{
    capture::{self, Capture, Monitor, WindowCache},
    detection::{Bar, Detector, Region, suppress},
    native::{self, IDLE, LIVE, Mapping, PAUSED, RECOVERING, STOP, Shared},
    overlay::Overlay,
    scanning::Scanner,
    tracking::Tracker,
};
use anyhow::Result;
use std::{
    ptr::null_mut,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct Screen {
    capture: Capture,
    overlay: Overlay,
    tracker: Tracker,
    scanner: Scanner,
    next_overview: Instant,
    next_face_check: Instant,
    face_cursor: usize,
    bars: Vec<Bar>,
}
impl Screen {
    fn new(m: Monitor, mode: u32) -> Result<Self> {
        Ok(Self {
            capture: Capture::new(m)?,
            overlay: Overlay::new(m)?,
            tracker: Tracker::default(),
            scanner: Scanner::new(m.w, m.h, mode),
            next_overview: Instant::now(),
            next_face_check: Instant::now(),
            face_cursor: 0,
            bars: Vec::new(),
        })
    }
}
fn face_region(f: &crate::detection::Face, w: usize, h: usize) -> Region {
    let x = (f.x - f.w * 0.35).floor().clamp(0., w as f32 - 1.) as usize;
    let y = (f.y - f.h * 0.35).floor().clamp(0., h as f32 - 1.) as usize;
    let r = (f.x + f.w * 1.35).ceil().clamp(x as f32 + 1., w as f32) as usize;
    let b = (f.y + f.h * 1.35).ceil().clamp(y as f32 + 1., h as f32) as usize;
    Region {
        x,
        y,
        w: r - x,
        h: b - y,
    }
}
pub fn run(name: &str) -> Result<()> {
    let mapping = Mapping::open(name)?;
    let shared = mapping.shared();
    shared.heartbeat.store(native::tick(), Ordering::Relaxed);
    native::log("worker initializing");
    let mut detector = Detector::new(&native::runtime())?;
    let mut screens: Vec<Screen> = Vec::new();
    let mut topology = Vec::new();
    let mut handles = Vec::new();
    let mut next_displays = Instant::now();
    let mut next_reassert = Instant::now();
    let mut last_loop = Instant::now();
    let mut was_paused = false;
    let mut last_options = (u32::MAX, u32::MAX);
    let mut all_bars = Vec::new();
    let mut window_cache = WindowCache::default();
    loop {
        let start = Instant::now();
        shared.heartbeat.store(native::tick(), Ordering::Relaxed);
        let flags = Shared::read(&shared.flags);
        if flags & STOP != 0 {
            break;
        }
        unsafe {
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if start.duration_since(last_loop) > Duration::from_secs(5) {
            topology.clear();
            next_displays = start;
        }
        last_loop = start;
        let mode = Shared::read(&shared.mode).min(2);
        if start >= next_displays {
            let monitors = capture::monitors()?;
            if monitors != topology {
                Shared::write(&shared.status, RECOVERING);
                screens.clear();
                all_bars.clear();
                handles.clear();
                for m in &monitors {
                    screens.push(Screen::new(*m, mode)?);
                }
                handles.extend(screens.iter().map(|s| s.overlay.handle()));
                native::log(&format!("display topology: {monitors:?}"));
                topology = monitors;
                Shared::write(&shared.monitors, screens.len() as u32);
            }
            next_displays = start + Duration::from_secs(2);
        }
        if flags & PAUSED != 0 {
            if !was_paused {
                for s in &mut screens {
                    s.overlay.paint(&[], true)?;
                    s.tracker = Tracker::default();
                }
                all_bars.clear();
                Shared::write(&shared.faces, 0);
            }
            was_paused = true;
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        let threshold = Shared::read(&shared.threshold).clamp(45, 90);
        let options_changed = (mode, threshold) != last_options || was_paused;
        was_paused = false;
        last_options = (mode, threshold);
        let (total_ms, frame_ms, face_ms) = match mode {
            1 => (52u64, 40, 90),
            2 => (14, 100, 220),
            _ => (30, 50, 120),
        };
        let budget = Duration::from_millis((total_ms / screens.len().max(1) as u64).max(10));
        let mut count = 0;
        let mut any_change = false;
        window_cache.begin_frame();
        for s in &mut screens {
            shared.heartbeat.store(native::tick(), Ordering::Relaxed);
            if options_changed {
                s.scanner = Scanner::new(s.capture.monitor.w, s.capture.monitor.h, mode);
                s.next_overview = start;
            }
            // Screenshots retain the bars. Only the private detector input
            // restores coherent visible regions from underlying window surfaces.
            s.capture
                .grab_clean(&all_bars, &handles, &mut window_cache)?;
            let image = s.capture.surface.image();
            let (changed, cut) = s.tracker.frame(&image, Instant::now());
            let tile_change = s.scanner.observe(&image, native::tick());
            any_change |= changed || tile_change;
            let scan_start = Instant::now();
            let mut found = Vec::new();
            let mut checked = Vec::new();
            if cut || (changed && scan_start >= s.next_overview) {
                let full = Region {
                    x: 0,
                    y: 0,
                    w: image.width,
                    h: image.height,
                };
                let faces = detector.detect(&image, full, threshold as f32 / 100.)?;
                found.extend(faces);
                checked.push((full, full.w.max(full.h) as f32 / 640. * 24.));
                s.next_overview = Instant::now() + Duration::from_millis(500);
            }
            // Local checks preserve landmark detail regardless of monitor shape.
            if changed
                && !s.tracker.tracks.is_empty()
                && Instant::now() >= s.next_face_check
                && scan_start.elapsed() < budget
            {
                let i = s.face_cursor % s.tracker.tracks.len();
                let region = face_region(&s.tracker.tracks[i].face, image.width, image.height);
                s.face_cursor = s.face_cursor.wrapping_add(1);
                let faces = detector.detect(&image, region, threshold as f32 / 100.)?;
                found.extend(faces);
                checked.push((region, 0.));
                s.next_face_check = Instant::now() + Duration::from_millis(face_ms);
            }
            let mut scans = 0;
            // A pending tile stays pending until scanned. There is no fixed
            // 280 ms wait between individual regions of an ultrawide monitor.
            while (scans == 0 || scan_start.elapsed() < budget)
                && let Some((i, region)) = s.scanner.next()
            {
                let faces = detector.detect(&image, region, threshold as f32 / 100.)?;
                found.extend(faces);
                checked.push((region, 0.));
                s.scanner.scanned(i, native::tick());
                scans += 1;
                shared.heartbeat.store(native::tick(), Ordering::Relaxed);
            }
            // Resolve overlapping scales before touching tracks. Updating after
            // every crop let the last, weaker crop overwrite better landmarks.
            let found = suppress(found);
            for (region, minimum) in checked {
                s.tracker.verify(&found, region, minimum);
            }
            s.tracker.update(found, Instant::now());
            s.bars.clear();
            for track in &s.tracker.tracks {
                let mut b = track.face.bar(
                    Shared::read(&shared.width) as f32 / 100.,
                    Shared::read(&shared.thickness) as f32 / 100.,
                );
                b.x += s.capture.monitor.x as f32;
                b.y += s.capture.monitor.y as f32;
                s.bars.push(b);
            }
            count += s.bars.len();
        }
        all_bars.clear();
        all_bars.extend(screens.iter().flat_map(|s| s.bars.iter().copied()));
        for s in &mut screens {
            s.overlay.paint(&all_bars, false)?;
            if start >= next_reassert {
                s.overlay.reassert();
            }
        }
        if start >= next_reassert {
            next_reassert = start + Duration::from_secs(1);
        }
        Shared::write(&shared.faces, count as u32);
        Shared::write(&shared.status, if any_change { LIVE } else { IDLE });
        Shared::write(
            &shared.cycle_ms,
            start.elapsed().as_millis().min(u32::MAX as u128) as u32,
        );
        shared.heartbeat.store(native::tick(), Ordering::Relaxed);
        let target = Duration::from_millis(if any_change {
            frame_ms
        } else {
            frame_ms.max(80)
        });
        if let Some(remaining) = target.checked_sub(start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
    Ok(())
}
