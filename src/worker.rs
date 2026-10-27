use crate::{
    analysis::{Analyzer, Job},
    capture::{self, Capture, Monitor, WindowCache},
    detection::{Bar, Image, Region},
    native::{self, IDLE, LIVE, Mapping, PAUSED, RECOVERING, STOP, Shared},
    overlay::Overlay,
    scanning::Scanner,
    tracking::Tracker,
};
use anyhow::{Result, ensure};
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
    scan_round: u64,
    changed: bool,
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
            scan_round: 0,
            changed: true,
            bars: Vec::new(),
        })
    }
}
pub(crate) fn face_region(f: &crate::detection::Face, w: usize, h: usize) -> Region {
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
    let analyzer = Analyzer::new()?;
    let mut inflight: Option<Instant> = None;
    let mut reusable_pixels: Option<Vec<u8>> = None;
    let mut generation = 0u64;
    let mut screen_cursor = 0usize;
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
    let mut next_trace = Instant::now();
    loop {
        let start = Instant::now();
        let trace = native::tracing();
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
                generation = generation.wrapping_add(1);
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
            if let Some(done) = analyzer.poll()? {
                inflight = None;
                reusable_pixels = Some(done.job.pixels);
            }
            ensure!(
                inflight.is_none_or(|since| since.elapsed() < Duration::from_secs(5)),
                "inference stalled while paused"
            );
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
        let (frame_ms, face_ms) = match mode {
            1 => (40, 90),
            2 => (100, 220),
            _ => (50, 120),
        };
        if options_changed {
            generation = generation.wrapping_add(1);
        }
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
            let before = s.tracker.tracks.len();
            let (changed, cut) = s.tracker.frame(&image, Instant::now());
            if trace && (cut || before != s.tracker.tracks.len()) {
                native::log(&format!(
                    "trace motion monitor={:?} changed={changed} cut={cut} tracks={before}->{}",
                    s.capture.monitor,
                    s.tracker.tracks.len()
                ));
            }
            let tile_change = s.scanner.observe(&image, native::tick());
            s.changed = changed || tile_change;
            any_change |= changed || tile_change;
            if cut {
                generation = generation.wrapping_add(1);
                s.next_overview = start;
            }
        }
        if let Some(done) = analyzer.poll()? {
            inflight = None;
            let Job {
                monitor,
                generation: revision,
                pixels,
                region,
                minimum,
                tile,
                captured,
                ..
            } = done.job;
            if revision == generation
                && captured.elapsed() < Duration::from_millis(750)
                && let Some(s) = screens.iter_mut().find(|s| s.capture.monitor == monitor)
            {
                let source = Image {
                    pixels: &pixels,
                    width: monitor.w,
                    height: monitor.h,
                };
                if let Some(found) = s.tracker.project(&source, done.faces) {
                    let before = s.tracker.tracks.len();
                    let detected = found.len();
                    s.tracker.verify(&found, region, minimum);
                    let verified = s.tracker.tracks.len();
                    s.tracker.update(found, Instant::now());
                    if trace {
                        native::log(&format!(
                            "trace scan region={region:?} age_ms={} detections={detected} tracks={before}->{verified}->{}",
                            captured.elapsed().as_millis(),
                            s.tracker.tracks.len()
                        ));
                    }
                    if let Some((i, token)) = tile {
                        s.scanner.completed(i, token, native::tick());
                    }
                }
            }
            reusable_pixels = Some(pixels);
        }
        ensure!(
            inflight.is_none_or(|since| since.elapsed() < Duration::from_secs(5)),
            "inference stalled"
        );
        if inflight.is_none() {
            // Round-robin monitors and scan kinds so one animated window cannot
            // monopolize inference. Capture/tracking never wait for this job.
            for offset in 0..screens.len() {
                let index = (screen_cursor + offset) % screens.len();
                let s = &mut screens[index];
                let m = s.capture.monitor;
                let full = Region {
                    x: 0,
                    y: 0,
                    w: m.w,
                    h: m.h,
                };
                let overview = s.changed
                    && start >= s.next_overview
                    && (s.scan_round.is_multiple_of(3) || s.tracker.tracks.is_empty());
                let roi = s.changed
                    && !s.tracker.tracks.is_empty()
                    && start >= s.next_face_check
                    && s.scan_round % 3 == 1;
                let selection = if overview {
                    s.next_overview = start + Duration::from_millis(500);
                    Some((full, full.w.max(full.h) as f32 / 640. * 24., None))
                } else if roi {
                    let i = s.face_cursor % s.tracker.tracks.len();
                    s.face_cursor = s.face_cursor.wrapping_add(1);
                    s.next_face_check = start + Duration::from_millis(face_ms);
                    Some((face_region(&s.tracker.tracks[i].face, m.w, m.h), 0., None))
                } else {
                    s.scanner
                        .next()
                        .map(|(i, r)| (r, 0., Some((i, s.scanner.token(i)))))
                };
                if let Some((region, minimum, tile)) = selection {
                    let mut pixels = reusable_pixels.take().unwrap_or_default();
                    pixels.clear();
                    pixels.extend_from_slice(s.capture.surface.pixels());
                    analyzer.submit(Job {
                        monitor: m,
                        generation,
                        pixels,
                        region,
                        minimum,
                        tile,
                        threshold: threshold as f32 / 100.,
                        mode,
                        captured: Instant::now(),
                    })?;
                    inflight = Some(Instant::now());
                    s.scan_round = s.scan_round.wrapping_add(1);
                    screen_cursor = (index + 1) % screens.len();
                    break;
                }
            }
        }
        for s in &mut screens {
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
        if trace && start >= next_trace {
            for s in &screens {
                let state: Vec<_> = s
                    .tracker
                    .tracks
                    .iter()
                    .take(8)
                    .map(|t| {
                        (
                            t.face.bar(1., 1.),
                            t.face.eye_count,
                            t.face.score,
                            t.face.detail,
                        )
                    })
                    .collect();
                native::log(&format!(
                    "trace frame mode={mode} threshold={threshold} monitor={:?} changed={} cycle_ms={} bars={} state={state:?}",
                    s.capture.monitor,
                    s.changed,
                    start.elapsed().as_millis(),
                    s.bars.len()
                ));
            }
            next_trace = start + Duration::from_millis(250);
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
