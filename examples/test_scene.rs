//! Optional manual desktop test fixture, not part of the shipped app.
//! cargo run --release --example test_scene -- path/to/reference-face.jpg
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

struct Tile {
    pixels: Vec<u8>,
    size: i32,
    x: i32,
    y: i32,
}
static TILES: OnceLock<Vec<Tile>> = OnceLock::new();
static mut PHASE: f32 = 0.;
static mut MOVING: bool = false;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut rect = zeroed();
                GetClientRect(hwnd, &mut rect);
                FillRect(dc, &rect, GetStockObject(WHITE_BRUSH) as HBRUSH);
                let title = wide("Pyra test: four face sizes. SPACE toggles motion. ESC closes.");
                TextOutW(dc, 20, 15, title.as_ptr(), title.len() as i32 - 1);
                let shift = (PHASE.sin() * 50.) as i32;
                for tile in TILES.get().unwrap() {
                    let info = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: tile.size,
                            biHeight: -tile.size,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB,
                            ..zeroed()
                        },
                        ..zeroed()
                    };
                    StretchDIBits(
                        dc,
                        tile.x + shift,
                        tile.y,
                        tile.size,
                        tile.size,
                        0,
                        0,
                        tile.size,
                        tile.size,
                        tile.pixels.as_ptr().cast(),
                        &info,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                }
                EndPaint(hwnd, &ps);
                return 0;
            }
            WM_TIMER => {
                if MOVING {
                    PHASE += 0.08;
                    InvalidateRect(hwnd, null(), 0);
                }
                return 0;
            }
            WM_KEYDOWN => {
                if w == 32 {
                    MOVING = !MOVING;
                }
                if w == 27 {
                    DestroyWindow(hwnd);
                }
                return 0;
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("provide reference image"))?;
    let source = image::open(path)?.to_rgba8();
    let mut tiles = Vec::new();
    for (x, y, size) in [
        (80, 60, 400),
        (560, 60, 240),
        (580, 400, 160),
        (820, 420, 128),
    ] {
        let im =
            image::imageops::resize(&source, size, size, image::imageops::FilterType::Triangle);
        let pixels = im.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        tiles.push(Tile {
            pixels,
            size: size as i32,
            x,
            y,
        });
    }
    TILES.set(tiles).ok();
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let name = wide("Pyra.TestScene");
        let module = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: module,
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("Pyra test scene").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            80,
            1100,
            720,
            null_mut(),
            null_mut(),
            module,
            null(),
        );
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, 1, 50, None);
        let mut message = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}
