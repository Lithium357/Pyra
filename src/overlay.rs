use crate::{
    capture::{Monitor, Surface},
    detection::Bar,
    native::wide,
};
use anyhow::{Result, ensure};
use std::{
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

pub struct Overlay {
    hwnd: HWND,
    surface: Surface,
    monitor: Monitor,
    previous: Vec<Bar>,
}

impl Overlay {
    pub fn handle(&self) -> HWND {
        self.hwnd
    }
    pub fn new(monitor: Monitor) -> Result<Self> {
        static CLASS: OnceLock<u16> = OnceLock::new();
        let name = wide("Pyra.EyeOverlay");
        let instance = unsafe { GetModuleHandleW(null()) };
        CLASS.get_or_init(|| unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc)
        });
        let surface = Surface::new(monitor.w, monitor.h)?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST,
                name.as_ptr(),
                wide("Pyra eye bars").as_ptr(),
                WS_POPUP,
                monitor.x,
                monitor.y,
                monitor.w as i32,
                monitor.h as i32,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        ensure!(!hwnd.is_null(), "create overlay window");
        unsafe {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let mut out = Self {
            hwnd,
            surface,
            monitor,
            previous: vec![],
        };
        out.paint(&[], true)?;
        Ok(out)
    }

    pub fn reassert(&self) {
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                self.monitor.x,
                self.monitor.y,
                self.monitor.w as i32,
                self.monitor.h as i32,
                SWP_NOACTIVATE,
            );
        }
    }

    pub fn paint(&mut self, bars: &[Bar], force: bool) -> Result<()> {
        if !force && bars == self.previous {
            return Ok(());
        }
        let w = self.surface.w;
        let h = self.surface.h;
        let pixels = self.surface.pixels_mut();
        pixels.fill(0);
        for bar in bars {
            let cx = bar.x - self.monitor.x as f32;
            let cy = bar.y - self.monitor.y as f32;
            let (s, c) = bar.angle.sin_cos();
            let ex = (bar.w * c.abs() + bar.h * s.abs()) * 0.5;
            let ey = (bar.w * s.abs() + bar.h * c.abs()) * 0.5;
            let left = ((cx - ex).floor() as i32).clamp(0, w as i32) as usize;
            let right = ((cx + ex).ceil() as i32).clamp(0, w as i32) as usize;
            let top = ((cy - ey).floor() as i32).clamp(0, h as i32) as usize;
            let bottom = ((cy + ey).ceil() as i32).clamp(0, h as i32) as usize;
            for y in top..bottom {
                let dy = y as f32 + 0.5 - cy;
                for x in left..right {
                    let dx = x as f32 + 0.5 - cx;
                    if (dx * c + dy * s).abs() <= bar.w * 0.5
                        && (-dx * s + dy * c).abs() <= bar.h * 0.5
                    {
                        pixels[y * w + x] = 0xff000000;
                    }
                }
            }
        }
        let position = POINT {
            x: self.monitor.x,
            y: self.monitor.y,
        };
        let origin = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: w as i32,
            cy: h as i32,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            ensure!(
                UpdateLayeredWindow(
                    self.hwnd,
                    null_mut(),
                    &position,
                    &size,
                    self.surface.dc,
                    &origin,
                    0,
                    &blend,
                    ULW_ALPHA
                ) != 0,
                "update overlay: {}",
                std::io::Error::last_os_error()
            );
        }
        self.previous.clear();
        self.previous.extend_from_slice(bars);
        Ok(())
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an unlocked Windows desktop"]
    fn ultrawide_and_portrait_overlay_pixels_use_physical_negative_origins() -> Result<()> {
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        for m in [
            Monitor {
                x: -5120,
                y: -200,
                w: 5120,
                h: 1440,
            },
            Monitor {
                x: 2000,
                y: -3840,
                w: 1080,
                h: 3840,
            },
            Monitor {
                x: -3440,
                y: 1200,
                w: 3440,
                h: 1440,
            },
        ] {
            let mut overlay = Overlay::new(m)?;
            let bars = [
                Bar {
                    x: m.x as f32 + 40.,
                    y: m.y as f32 + 40.,
                    w: 60.,
                    h: 20.,
                    angle: 0.,
                },
                Bar {
                    x: m.x as f32 + m.w as f32 - 40.,
                    y: m.y as f32 + m.h as f32 - 40.,
                    w: 60.,
                    h: 20.,
                    angle: 0.,
                },
            ];
            overlay.paint(&bars, true)?;
            overlay.reassert();
            let mut rect: RECT = unsafe { std::mem::zeroed() };
            unsafe {
                GetWindowRect(overlay.hwnd, &mut rect);
            }
            ensure!(
                rect.left == m.x
                    && rect.top == m.y
                    && rect.right - rect.left == m.w as i32
                    && rect.bottom - rect.top == m.h as i32,
                "physical coordinates were virtualized or clipped"
            );
            let pixels = overlay.surface.pixels_mut();
            ensure!(
                pixels[40 * m.w + 40] == 0xff000000,
                "near-edge bar is misplaced"
            );
            ensure!(
                pixels[(m.h - 40) * m.w + m.w - 40] == 0xff000000,
                "far-edge bar is misplaced"
            );
            ensure!(
                pixels[(m.h / 2) * m.w + m.w / 2] == 0,
                "transparent space was filled"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires the local Pyra browser coverage test window and PYRA_RUNTIME"]
    fn browser_window_surface_captures_all_four_faces() -> Result<()> {
        use crate::{
            capture::Surface,
            detection::{Detector, Region},
        };
        unsafe {
            let hwnd = FindWindowW(
                null(),
                wide("Pyra browser coverage test - Google Chrome").as_ptr(),
            );
            ensure!(!hwnd.is_null(), "open the local browser fixture first");
            let mut rect: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            let surface = Surface::new(
                (rect.right - rect.left) as usize,
                (rect.bottom - rect.top) as usize,
            )?;
            let mut detector = Detector::new(&crate::native::runtime())?;
            let dc = GetWindowDC(hwnd);
            BitBlt(
                surface.dc,
                0,
                0,
                surface.w as i32,
                surface.h as i32,
                dc,
                0,
                0,
                SRCCOPY,
            );
            ReleaseDC(hwnd, dc);
            GdiFlush();
            let full = Region {
                x: 0,
                y: 0,
                w: surface.w,
                h: surface.h,
            };
            let gdi = detector.detect(&surface.image(), full, 0.65)?.len();
            let begin = std::time::Instant::now();
            let success = windows_sys::Win32::Storage::Xps::PrintWindow(hwnd, surface.dc, 2);
            GdiFlush();
            let elapsed = begin.elapsed();
            let mut detections = detector.detect(&surface.image(), full, 0.65)?;
            for tile in crate::scanning::grid(surface.w, surface.h, 800) {
                detections.extend(detector.detect(&surface.image(), tile, 0.65)?);
            }
            let rendered = crate::detection::suppress(detections).len();
            println!(
                "Chrome: window DC faces={gdi}; PrintWindow faces={rendered}; render={elapsed:?}; success={success}"
            );
            ensure!(
                rendered == 4,
                "browser render did not capture all four faces"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires an unlocked Windows desktop"]
    fn ordinary_capture_sees_bars_but_detector_capture_sees_background() -> Result<()> {
        use crate::capture::Capture;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmFlush() -> i32;
        }
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let monitor = crate::capture::monitors()?[0];
            let region = Monitor {
                x: monitor.x + 100,
                y: monitor.y + 100,
                w: 240,
                h: 120,
            };
            let class = wide("Pyra.CaptureTestBackdrop");
            let instance = GetModuleHandleW(null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                hbrBackground: GetStockObject(WHITE_BRUSH) as HBRUSH,
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let backdrop = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                wide("Pyra capture test").as_ptr(),
                WS_POPUP,
                region.x,
                region.y,
                region.w as i32,
                region.h as i32,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            ensure!(!backdrop.is_null(), "create capture-test backdrop");
            struct Backdrop(HWND);
            impl Drop for Backdrop {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let _backdrop = Backdrop(backdrop);
            ShowWindow(backdrop, SW_SHOWNOACTIVATE);
            UpdateWindow(backdrop);
            DwmFlush();
            let mut overlay = Overlay::new(region)?;
            let bars = [Bar {
                x: region.x as f32 + 120.,
                y: region.y as f32 + 60.,
                w: 140.,
                h: 30.,
                angle: 0.,
            }];
            overlay.paint(&bars, true)?;
            DwmFlush();
            std::thread::sleep(std::time::Duration::from_millis(80));
            let mut capture = Capture::new(region)?;
            let center = (60 * region.w + 120) * 4;
            capture.grab_with_layers(true)?;
            let ordinary = capture.surface.pixels()[center..center + 3].to_vec();
            capture.grab_clean(
                &bars,
                &[overlay.handle()],
                &mut crate::capture::WindowCache::default(),
            )?;
            let detector = capture.surface.pixels()[center..center + 3].to_vec();
            println!("ordinary screenshot={ordinary:?}; clean detector input={detector:?}");
            ensure!(
                ordinary == [0, 0, 0],
                "standard screenshot omitted the bar: {ordinary:?}"
            );
            ensure!(
                detector == [255, 255, 255],
                "detector sees its own bar: {detector:?}"
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "requires an unlocked Windows desktop"]
    fn native_overlay_capture_visibility_geometry_and_resource_cleanup() -> Result<()> {
        use windows_sys::Win32::System::Threading::{
            GR_GDIOBJECTS, GetCurrentProcess, GetGuiResources,
        };
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let monitor = crate::capture::monitors()?[0];
            let before = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            for _ in 0..25 {
                let mut overlay = Overlay::new(monitor)?;
                let mut affinity = 0;
                ensure!(
                    GetWindowDisplayAffinity(overlay.hwnd, &mut affinity) != 0
                        && affinity == WDA_NONE,
                    "overlay is hidden from screenshots"
                );
                ensure!(
                    GetWindowLongPtrW(overlay.hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT != 0,
                    "overlay intercepts input"
                );
                let mut rect: RECT = std::mem::zeroed();
                GetWindowRect(overlay.hwnd, &mut rect);
                ensure!(
                    rect.left == monitor.x
                        && rect.top == monitor.y
                        && rect.right - rect.left == monitor.w as i32
                        && rect.bottom - rect.top == monitor.h as i32,
                    "overlay physical geometry mismatch"
                );
                overlay.paint(
                    &[Bar {
                        x: 50.,
                        y: 50.,
                        w: 100.,
                        h: 25.,
                        angle: 0.2,
                    }],
                    false,
                )?;
            }
            GdiFlush();
            let after = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            ensure!(
                after <= before + 2,
                "GDI resources leaked: before={before} after={after}"
            );
            println!("25 overlay lifecycles: GDI handles before={before}, after={after}");
        }
        Ok(())
    }
}
