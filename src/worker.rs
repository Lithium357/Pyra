use crate::{
    capture::{self, Capture, Monitor},
    detection::{Bar, Detector, Region, tiles},
    native::{self, IDLE, LIVE, Mapping, PAUSED, RECOVERING, STOP, Shared},
    overlay::Overlay,
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
    next_detect: Instant,
    next_tile: Instant,
    tile: usize,
    initial_tiles: usize,
    regions: Vec<Region>,
    bars: Vec<Bar>,
    dirty: bool,
}
impl Screen {
    fn new(monitor: Monitor) -> Result<Self> {
        Ok(Self {
            capture: Capture::new(monitor)?,
            overlay: Overlay::new(monitor)?,
            tracker: Tracker::default(),
            next_detect: Instant::now(),
            next_tile: Instant::now(),
            tile: 0,
            initial_tiles: 0,
            regions: tiles(monitor.w, monitor.h),
            bars: Vec::new(),
            dirty: true,
        })
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
    let mut next_displays = Instant::now();
    let mut next_reassert = Instant::now();
    let mut last_loop = Instant::now();
    let mut was_paused = false;
    let mut last_options = (0, 65);
    let mut all_bars = Vec::new();
    loop {
        let start = Instant::now();
        shared.heartbeat.store(native::tick(), Ordering::Relaxed);
        let flags = Shared::read(&shared.flags);
        if flags & STOP != 0 {
            break;
        }
        // Worker owns all overlay HWNDs and their message queue.
        unsafe {
            let mut message = std::mem::zeroed();
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    return Ok(());
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if start.duration_since(last_loop) > Duration::from_secs(5) {
            // Recreate GDI/DC resources after sleep or a GPU/display reset.
            topology.clear();
            next_displays = start;
        }
        last_loop = start;
        if start >= next_displays {
            let monitors = capture::monitors()?;
            if monitors != topology {
                Shared::write(&shared.status, RECOVERING);
                screens.clear();
                for monitor in &monitors {
                    screens.push(Screen::new(*monitor)?);
                }
                native::log(&format!("display topology: {monitors:?}"));
                topology = monitors;
                Shared::write(&shared.monitors, screens.len() as u32);
            }
            next_displays = start + Duration::from_secs(2);
        }
        if flags & PAUSED != 0 {
            if !was_paused {
                for screen in &mut screens {
                    screen.overlay.paint(&[], true)?;
                    screen.tracker = Tracker::default();
                    screen.initial_tiles = 0;
                }
                Shared::write(&shared.faces, 0);
            }
            was_paused = true;
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if was_paused {
            for screen in &mut screens {
                screen.next_detect = start;
                screen.next_tile = start;
            }
            was_paused = false;
        }
        let mode = Shared::read(&shared.mode).min(2);
        let threshold = Shared::read(&shared.threshold).clamp(45, 90);
        let options = (mode, threshold);
        let changed_options = options != last_options;
        last_options = options;
        let (detect_ms, tile_ms, frame_ms) = match mode {
            1 => (80, 100, 40),
            2 => (220, 1000, 100),
            _ => (130, 280, 50),
        };
        let mut count = 0;
        let mut any_change = false;
        for screen in &mut screens {
            shared.heartbeat.store(native::tick(), Ordering::Relaxed);
            if changed_options {
                screen.initial_tiles = 0;
                screen.next_detect = start;
            }
            screen.capture.grab()?;
            let image = screen.capture.surface.image();
            let now = Instant::now();
            let (changed, cut) = screen.tracker.frame(&image, now);
            if changed || changed_options {
                screen.dirty = true;
                screen.initial_tiles = 0;
            }
            any_change |= changed;
            if cut
                || changed_options
                || (screen.dirty && now >= screen.next_detect)
                || now.duration_since(screen.next_detect.min(now)) > Duration::from_secs(2)
            {
                let region = Region {
                    x: 0,
                    y: 0,
                    w: image.width,
                    h: image.height,
                };
                let faces = detector.detect(&image, region, threshold as f32 / 100.)?;
                screen
                    .tracker
                    .verify(&faces, region, region.w.max(region.h) as f32 / 640. * 24.);
                screen.tracker.update(faces, Instant::now());
                screen.dirty = false;
                screen.next_detect = Instant::now() + Duration::from_millis(detect_ms);
            }
            if mode != 2
                && Instant::now() >= screen.next_tile
                && (changed || screen.initial_tiles < screen.regions.len())
            {
                let region = screen.regions[screen.tile % screen.regions.len()];
                let faces = detector.detect(&image, region, threshold as f32 / 100.)?;
                screen.tracker.verify(&faces, region, 0.);
                screen.tracker.update(faces, Instant::now());
                screen.tile = (screen.tile + 1) % screen.regions.len();
                screen.initial_tiles = (screen.initial_tiles + 1).min(screen.regions.len());
                screen.next_tile = Instant::now() + Duration::from_millis(tile_ms);
            }
            screen.bars.clear();
            for track in &screen.tracker.tracks {
                let mut bar = track.face.bar(
                    Shared::read(&shared.width) as f32 / 100.,
                    Shared::read(&shared.thickness) as f32 / 100.,
                );
                bar.x += screen.capture.monitor.x as f32;
                bar.y += screen.capture.monitor.y as f32;
                screen.bars.push(bar);
            }
            count += screen.bars.len();
        }
        // Paint every monitor from the same latest set. This also covers a bar
        // straddling a display boundary without physical/logical DPI conversion.
        all_bars.clear();
        all_bars.extend(screens.iter().flat_map(|s| s.bars.iter().copied()));
        for screen in &mut screens {
            screen.overlay.paint(&all_bars, false)?;
            if start >= next_reassert {
                screen.overlay.reassert();
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
            frame_ms.max(100)
        });
        if let Some(remaining) = target.checked_sub(start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
    Ok(())
}
