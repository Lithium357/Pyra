use crate::detection::{Bar, Image};
use anyhow::{Result, bail, ensure};
use std::{
    ffi::c_void,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
}

unsafe extern "system" fn enum_monitor(_: HMONITOR, _: HDC, rect: *mut RECT, data: LPARAM) -> i32 {
    unsafe {
        let r = &*rect;
        let out = &mut *(data as *mut Vec<Monitor>);
        if r.right > r.left && r.bottom > r.top {
            out.push(Monitor {
                x: r.left,
                y: r.top,
                w: (r.right - r.left) as usize,
                h: (r.bottom - r.top) as usize,
            });
        }
    }
    1
}

pub fn monitors() -> Result<Vec<Monitor>> {
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        ensure!(
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_monitor),
                &mut out as *mut _ as LPARAM
            ) != 0,
            "enumerate displays: {}",
            std::io::Error::last_os_error()
        );
    }
    ensure!(!out.is_empty(), "no active displays");
    out.sort_by_key(|m| (m.x, m.y));
    out.dedup();
    Ok(out)
}

pub struct Surface {
    pub dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: usize,
    pub h: usize,
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Result<Self> {
        ensure!(
            w > 0
                && h > 0
                && w <= 32768
                && h <= 32768
                && w.checked_mul(h).is_some_and(|n| n <= 100_000_000),
            "display dimensions exceed safety limit"
        );
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            ensure!(!dc.is_null(), "create image DC");
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                bail!("allocate display image");
            }
            let old = SelectObject(dc, bitmap);
            if old.is_null() || old as isize == -1 {
                DeleteObject(bitmap);
                DeleteDC(dc);
                bail!("select image into DC");
            }
            Ok(Self {
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                w,
                h,
            })
        }
    }
    pub fn pixels(&self) -> &[u8] {
        // DIB allocation has exactly w*h 32-bit, tightly packed top-down pixels.
        unsafe { std::slice::from_raw_parts(self.bits, self.w * self.h * 4) }
    }
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits.cast(), self.w * self.h) }
    }
    pub fn image(&self) -> Image<'_> {
        Image {
            pixels: self.pixels(),
            width: self.w,
            height: self.h,
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub struct Capture {
    pub surface: Surface,
    pub monitor: Monitor,
    desktop: HDC,
}

struct WindowBitmap {
    hwnd: HWND,
    surface: Surface,
    frame: u64,
    used: u64,
}

/// Shared by monitor captures, so a window crossing a monitor seam is rendered
/// once per frame. Four reusable surfaces bound the retained window history.
#[derive(Default)]
pub struct WindowCache {
    entries: Vec<WindowBitmap>,
    frame: u64,
}
impl WindowCache {
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.entries
            .retain(|e| unsafe { IsWindow(e.hwnd) != 0 } && self.frame.wrapping_sub(e.used) < 90);
    }
    unsafe fn render(&mut self, hwnd: HWND, bounds: &RECT) -> Option<HDC> {
        unsafe {
            let (w, h) = (
                (bounds.right - bounds.left) as usize,
                (bounds.bottom - bounds.top) as usize,
            );
            let index = if let Some(i) = self.entries.iter().position(|e| e.hwnd == hwnd) {
                i
            } else {
                if self.entries.len() >= 4 {
                    let oldest = self
                        .entries
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, e)| e.used)?
                        .0;
                    self.entries.swap_remove(oldest);
                }
                self.entries.push(WindowBitmap {
                    hwnd,
                    surface: Surface::new(w, h).ok()?,
                    frame: u64::MAX,
                    used: self.frame,
                });
                self.entries.len() - 1
            };
            let entry = &mut self.entries[index];
            entry.used = self.frame;
            if entry.surface.w != w || entry.surface.h != h {
                entry.surface = Surface::new(w, h).ok()?;
                entry.frame = u64::MAX;
            }
            if entry.frame != self.frame {
                let mut result = 0;
                // Avoid starting a synchronous render against an app already
                // known to be unresponsive. The process watchdog covers a driver
                // or application that stalls after this check.
                if SendMessageTimeoutW(
                    hwnd,
                    WM_NULL,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    20,
                    &mut result,
                ) == 0
                {
                    return None;
                }
                entry.surface.pixels_mut().fill(0);
                let saved = SaveDC(entry.surface.dc);
                let success =
                    windows_sys::Win32::Storage::Xps::PrintWindow(hwnd, entry.surface.dc, 2);
                RestoreDC(entry.surface.dc, saved);
                GdiFlush();
                if success == 0 {
                    return None;
                }
                entry.frame = self.frame;
            }
            Some(entry.surface.dc)
        }
    }
}
impl Capture {
    pub fn new(monitor: Monitor) -> Result<Self> {
        let surface = Surface::new(monitor.w, monitor.h)?;
        let desktop = unsafe { GetDC(null_mut()) };
        ensure!(!desktop.is_null(), "open desktop DC");
        Ok(Self {
            surface,
            monitor,
            desktop,
        })
    }
    pub fn grab(&mut self) -> Result<()> {
        self.grab_with_layers(true)
    }
    /// Capture normally (so other screen tools see our bars), then replace each
    /// affected window's visible region with a coherent, overlay-free frame.
    /// The visible-region subtraction preserves foreground-window occlusion.
    pub fn grab_clean(
        &mut self,
        bars: &[Bar],
        overlays: &[HWND],
        cache: &mut WindowCache,
    ) -> Result<()> {
        self.grab()?;
        self.restore_overlay_areas(bars, overlays, cache)
    }

    pub fn restore_overlay_areas(
        &mut self,
        bars: &[Bar],
        overlays: &[HWND],
        cache: &mut WindowCache,
    ) -> Result<()> {
        if bars.is_empty() {
            return Ok(());
        }
        unsafe {
            let m = self.monitor;
            let remaining = RegionHandle(CreateRectRgn(0, 0, 0, 0));
            ensure!(
                !remaining.0.is_null(),
                "allocate overlay restoration region"
            );
            for bar in bars {
                let (sin, cos) = bar.angle.sin_cos();
                let ex = (bar.w * cos.abs() + bar.h * sin.abs()) * 0.5 + 2.;
                let ey = (bar.w * sin.abs() + bar.h * cos.abs()) * 0.5 + 2.;
                let left = (bar.x - ex).floor().max(m.x as f32) as i32;
                let top = (bar.y - ey).floor().max(m.y as f32) as i32;
                let right = (bar.x + ex).ceil().min(m.x as f32 + m.w as f32) as i32;
                let bottom = (bar.y + ey).ceil().min(m.y as f32 + m.h as f32) as i32;
                if left < right && top < bottom {
                    let rect = RegionHandle(CreateRectRgn(left, top, right, bottom));
                    CombineRgn(remaining.0, remaining.0, rect.0, RGN_OR);
                }
            }
            let mut context = RestoreContext {
                remaining: remaining.0,
                overlays,
                dc: self.surface.dc,
                origin: (m.x, m.y),
                cache,
                uncovered: CreateRectRgn(m.x, m.y, m.x + m.w as i32, m.y + m.h as i32),
            };
            let _uncovered = RegionHandle(context.uncovered);
            ensure!(
                !context.uncovered.is_null(),
                "allocate coherent capture visibility region"
            );
            EnumWindows(Some(restore_window), &mut context as *mut _ as LPARAM);
            SelectClipRgn(self.surface.dc, null_mut());
            GdiFlush();
        }
        Ok(())
    }
    pub fn grab_with_layers(&mut self, include_layers: bool) -> Result<()> {
        let m = self.monitor;
        unsafe {
            ensure!(
                BitBlt(
                    self.surface.dc,
                    0,
                    0,
                    m.w as i32,
                    m.h as i32,
                    self.desktop,
                    m.x,
                    m.y,
                    SRCCOPY | if include_layers { CAPTUREBLT } else { 0 }
                ) != 0,
                "screen capture failed: {}",
                std::io::Error::last_os_error()
            );
            GdiFlush();
        }
        Ok(())
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(null_mut(), self.desktop);
        }
    }
}

struct RegionHandle(HRGN);
impl Drop for RegionHandle {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

struct RestoreContext<'a> {
    remaining: HRGN,
    overlays: &'a [HWND],
    dc: HDC,
    origin: (i32, i32),
    cache: &'a mut WindowCache,
    uncovered: HRGN,
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(hwnd: HWND, attribute: u32, value: *mut c_void, size: u32) -> i32;
}

unsafe extern "system" fn restore_window(hwnd: HWND, data: LPARAM) -> i32 {
    unsafe {
        let context = &mut *(data as *mut RestoreContext<'_>);
        if context.overlays.contains(&hwnd) || IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        // Click-through layered utility windows (cursor/highlight overlays,
        // for example) do not occlude their whole rectangular bounds. Their
        // window DC can contain the already-composited desktop, including our
        // bars. Look through them to the application's independent surface.
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if style & (WS_EX_LAYERED | WS_EX_TRANSPARENT) == (WS_EX_LAYERED | WS_EX_TRANSPARENT) {
            return 1;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, 14, (&mut cloaked as *mut u32).cast(), 4) == 0
            && cloaked != 0
        {
            return 1;
        }
        let mut bounds: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut bounds) == 0
            || bounds.right <= bounds.left
            || bounds.bottom <= bounds.top
        {
            return 1;
        }
        let window = RegionHandle(CreateRectRgn(
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom,
        ));
        let visible = RegionHandle(CreateRectRgn(0, 0, 0, 0));
        if window.0.is_null() || visible.0.is_null() {
            return 1;
        }
        if CombineRgn(visible.0, context.uncovered, window.0, RGN_AND) == NULLREGION {
            return 1;
        }
        let affected = RegionHandle(CreateRectRgn(0, 0, 0, 0));
        let needed = CombineRgn(affected.0, visible.0, context.remaining, RGN_AND) != NULLREGION;
        // Occluding foreground windows must remain intact even when they have
        // no bars. Consume their visible area before examining windows below.
        CombineRgn(context.uncovered, context.uncovered, visible.0, RGN_DIFF);
        if !needed {
            return 1;
        }
        // Replace this window's entire visible region from ONE source frame.
        // Patching just an eye strip mixed desktop time T0 with render time T1,
        // which let the overlay alter the tracker's apparent facial motion.
        let mut dc = GetWindowDC(hwnd);
        if dc.is_null() {
            return 1;
        }
        OffsetRgn(visible.0, -context.origin.0, -context.origin.1);
        SelectClipRgn(context.dc, visible.0);
        let mut copy: RECT = std::mem::zeroed();
        GetRgnBox(visible.0, &mut copy);
        let mut name = [0u16; 256];
        let count = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&name[..count]);
        let accelerated = class.starts_with("Chrome_")
            || class.starts_with("Qt")
            || class == "MozillaWindowClass"
            || class.starts_with("HwndWrapper")
            || class == "ApplicationFrameWindow"
            || class == "Windows.UI.Core.CoreWindow";
        let mut release = true;
        if accelerated && let Some(rendered) = context.cache.render(hwnd, &bounds) {
            ReleaseDC(hwnd, dc);
            dc = rendered;
            release = false;
        }
        let success = BitBlt(
            context.dc,
            copy.left,
            copy.top,
            copy.right - copy.left,
            copy.bottom - copy.top,
            dc,
            copy.left + context.origin.0 - bounds.left,
            copy.top + context.origin.1 - bounds.top,
            SRCCOPY,
        );
        SelectClipRgn(context.dc, null_mut());
        if release {
            ReleaseDC(hwnd, dc);
        }
        OffsetRgn(visible.0, context.origin.0, context.origin.1);
        if success != 0
            && CombineRgn(context.remaining, context.remaining, visible.0, RGN_DIFF) == NULLREGION
        {
            return 0;
        }
        1
    }
}
