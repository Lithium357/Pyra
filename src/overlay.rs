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
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                DestroyWindow(hwnd);
                anyhow::bail!(
                    "Cannot exclude overlay from capture. Windows 10 2004+ is required: {}",
                    std::io::Error::last_os_error()
                );
            }
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
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
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
    fn native_overlay_exclusion_geometry_and_resource_cleanup() -> Result<()> {
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
                        && affinity == WDA_EXCLUDEFROMCAPTURE,
                    "overlay not excluded from capture"
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
