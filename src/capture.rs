use crate::detection::Image;
use anyhow::{Result, bail, ensure};
use std::{
    ffi::c_void,
    mem::size_of,
    ptr::{null, null_mut},
};
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
                    SRCCOPY | CAPTUREBLT
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
