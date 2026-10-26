use crate::{capture::Surface, native::wide};
use anyhow::{Result, ensure};
use std::{ptr::null, sync::OnceLock};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*};

struct Mask {
    w: usize,
    h: usize,
    pixels: Vec<u8>,
}

pub struct Label {
    levels: Vec<Mask>,
}

pub struct FittedLabel<'a> {
    mask: &'a Mask,
    w: f32,
    h: f32,
}

impl Label {
    pub fn shared() -> Result<&'static Self> {
        static LABEL: OnceLock<Label> = OnceLock::new();
        if LABEL.get().is_none() {
            let _ = LABEL.set(Self::new()?);
        }
        Ok(LABEL.get().expect("initialized label"))
    }

    fn new() -> Result<Self> {
        // Load only into this process, rasterize once, then release the font.
        // Rendering bars never calls a font API or allocates a text bitmap.
        let bytes = include_bytes!("../assets/Doto.ttf");
        unsafe {
            let mut count = 0u32;
            let resource = AddFontMemResourceEx(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                null(),
                &raw mut count,
            );
            ensure!(!resource.is_null() && count > 0, "load embedded Doto font");
            struct FontResource(HANDLE);
            impl Drop for FontResource {
                fn drop(&mut self) {
                    unsafe {
                        RemoveFontMemResourceEx(self.0);
                    }
                }
            }
            let _resource = FontResource(resource);
            let font = CreateFontW(
                -96,
                0,
                0,
                0,
                700,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_TT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                wide("Doto").as_ptr(),
            );
            ensure!(!font.is_null(), "create Doto font");
            struct Font(HFONT);
            impl Drop for Font {
                fn drop(&mut self) {
                    unsafe {
                        DeleteObject(self.0);
                    }
                }
            }
            let _font = Font(font);
            let mut surface = Surface::new(1024, 160)?;
            surface.pixels_mut().fill(0);
            let old = SelectObject(surface.dc, font);
            let result = (|| -> Result<Self> {
                let mut name = [0u16; 128];
                let length = GetTextFaceW(surface.dc, name.len() as i32, name.as_mut_ptr());
                ensure!(
                    length > 0 && String::from_utf16_lossy(&name[..length as usize - 1]) == "Doto",
                    "embedded Doto font was substituted"
                );
                let text = wide("[censored]");
                let mut size: SIZE = std::mem::zeroed();
                ensure!(
                    GetTextExtentPoint32W(surface.dc, text.as_ptr(), 10, &mut size) != 0
                        && size.cx < 1024
                        && size.cy < 160,
                    "measure Doto label"
                );
                SetTextColor(surface.dc, 0x00ffffff);
                SetBkColor(surface.dc, 0);
                ensure!(
                    TextOutW(surface.dc, 0, 0, text.as_ptr(), 10) != 0,
                    "rasterize Doto label"
                );
                GdiFlush();
                let pixels = surface.pixels();
                let (mut left, mut top, mut right, mut bottom) = (1024, 160, 0, 0);
                for y in 0..160 {
                    for x in 0..1024 {
                        if pixels[(y * 1024 + x) * 4] > 0 {
                            left = left.min(x);
                            top = top.min(y);
                            right = right.max(x + 1);
                            bottom = bottom.max(y + 1);
                        }
                    }
                }
                ensure!(right > left && bottom > top, "empty Doto label");
                let (w, h) = (right - left, bottom - top);
                let mut ink = Vec::with_capacity(w * h);
                for y in top..bottom {
                    for x in left..right {
                        ink.push(pixels[(y * 1024 + x) * 4]);
                    }
                }
                let mut levels = vec![Mask { w, h, pixels: ink }];
                // Prefilter shrinking text so Doto's dots don't blink on and
                // off as bar scale changes. The pyramid is small and immutable.
                while levels.last().unwrap().h > 2 {
                    let previous = levels.last().unwrap();
                    let (w, h) = (previous.w.div_ceil(2), previous.h.div_ceil(2));
                    let mut pixels = Vec::with_capacity(w * h);
                    for y in 0..h {
                        for x in 0..w {
                            let mut sum = 0u32;
                            let mut count = 0;
                            for yy in y * 2..(y * 2 + 2).min(previous.h) {
                                for xx in x * 2..(x * 2 + 2).min(previous.w) {
                                    sum += previous.pixels[yy * previous.w + xx] as u32;
                                    count += 1;
                                }
                            }
                            pixels.push((sum / count) as u8);
                        }
                    }
                    levels.push(Mask { w, h, pixels });
                }
                Ok(Self { levels })
            })();
            SelectObject(surface.dc, old);
            result
        }
    }

    pub fn fit(&self, bar_w: f32, bar_h: f32) -> FittedLabel<'_> {
        let full = &self.levels[0];
        let scale = (bar_w * 0.88 / full.w as f32).min(bar_h * 0.68 / full.h as f32);
        let (w, h) = (full.w as f32 * scale, full.h as f32 * scale);
        let mut mask = full;
        for level in &self.levels[1..] {
            if level.h as f32 >= h {
                mask = level;
            } else {
                break;
            }
        }
        FittedLabel { mask, w, h }
    }
}

impl FittedLabel<'_> {
    pub fn shade(&self, x: f32, y: f32) -> u32 {
        let u = x / self.w + 0.5;
        let v = y / self.h + 0.5;
        if !(0. ..1.).contains(&u) || !(0. ..1.).contains(&v) {
            return 0;
        }
        let px = u * self.mask.w as f32 - 0.5;
        let py = v * self.mask.h as f32 - 0.5;
        let (ix, iy) = (px.floor() as i32, py.floor() as i32);
        let at = |x: i32, y: i32| -> f32 {
            if x < 0 || y < 0 || x >= self.mask.w as i32 || y >= self.mask.h as i32 {
                0.
            } else {
                self.mask.pixels[y as usize * self.mask.w + x as usize] as f32
            }
        };
        let (fx, fy) = (px - ix as f32, py - iy as f32);
        let a = at(ix, iy) * (1. - fx) + at(ix + 1, iy) * fx;
        let b = at(ix, iy + 1) * (1. - fx) + at(ix + 1, iy + 1) * fx;
        (a * (1. - fy) + b * fy).round() as u32
    }
}
