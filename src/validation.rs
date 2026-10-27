use crate::detection::{Detector, Face, Image, Region, suppress};
use anyhow::Result;
use image::{Rgb, RgbImage};
use std::{path::PathBuf, time::Instant};

// Optional native feedback fixture for a private wallpaper reproduction. It
// renders an image in our own window without changing the user's wallpaper.
struct StaticDesktop {
    hwnd: windows_sys::Win32::Foundation::HWND,
    capture: crate::capture::Capture,
    overlay: crate::overlay::Overlay,
    cache: crate::capture::WindowCache,
}
impl StaticDesktop {
    fn new(image: &Image<'_>) -> Result<Self> {
        use windows_sys::Win32::{
            Graphics::Gdi::UpdateWindow, System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::*,
        };
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            let screen = crate::capture::monitors()?[0];
            anyhow::ensure!(
                image.width <= screen.w && image.height <= screen.h,
                "private fixture must fit on the first display"
            );
            let m = crate::capture::Monitor {
                x: screen.x + (screen.w - image.width).min(20) as i32,
                y: screen.y + (screen.h - image.height).min(20) as i32,
                w: image.width,
                h: image.height,
            };
            let name: Vec<_> = "Pyra.StaticWallpaperTest"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                name.as_ptr(),
                name.as_ptr(),
                WS_POPUP,
                m.x,
                m.y,
                m.w as i32,
                m.h as i32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            anyhow::ensure!(!hwnd.is_null(), "create wallpaper feedback fixture");
            let mut scene = Self {
                hwnd,
                capture: crate::capture::Capture::new(m)?,
                overlay: crate::overlay::Overlay::new(m)?,
                cache: crate::capture::WindowCache::default(),
            };
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            UpdateWindow(hwnd);
            scene.present(image)?;
            Ok(scene)
        }
    }
    fn present(&mut self, image: &Image<'_>) -> Result<()> {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            let m = self.capture.monitor;
            anyhow::ensure!(
                image.width == m.w && image.height == m.h,
                "fixture size changed"
            );
            for (pixel, bytes) in self
                .capture
                .surface
                .pixels_mut()
                .iter_mut()
                .zip(image.pixels.as_chunks::<4>().0)
            {
                *pixel = u32::from_le_bytes(*bytes);
            }
            let dc = GetWindowDC(self.hwnd);
            let ok = BitBlt(
                dc,
                0,
                0,
                m.w as i32,
                m.h as i32,
                self.capture.surface.dc,
                0,
                0,
                SRCCOPY,
            );
            ReleaseDC(self.hwnd, dc);
            anyhow::ensure!(ok != 0, "draw wallpaper fixture");
            Ok(())
        }
    }
    fn frame(&mut self, tracker: &crate::tracking::Tracker) -> Result<Image<'_>> {
        let m = self.capture.monitor;
        let bars: Vec<_> = tracker
            .tracks
            .iter()
            .map(|t| {
                let mut b = t.face.bar(1., 1.);
                b.x += m.x as f32;
                b.y += m.y as f32;
                b
            })
            .collect();
        self.overlay.paint(&bars, true)?;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmFlush() -> i32;
        }
        unsafe {
            DwmFlush();
        }
        self.cache.begin_frame();
        self.capture
            .grab_clean(&bars, &[self.overlay.handle()], &mut self.cache)?;
        Ok(self.capture.surface.image())
    }
}
impl Drop for StaticDesktop {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

#[test]
#[ignore = "requires PYRA_RUNTIME and PYRA_CORPUS, or PYRA_STATIC_IMAGE for a private local reproduction"]
fn static_image_scan_consistency() -> Result<()> {
    let im = if let Some(path) = std::env::var_os("PYRA_STATIC_IMAGE") {
        image::open(path)?.to_rgb8()
    } else {
        let group =
            image::open(PathBuf::from(std::env::var("PYRA_CORPUS")?).join("generated-group.jpg"))?
                .to_rgb8();
        image::imageops::resize(
            &image::imageops::crop_imm(&group, 0, 0, 512, 512).to_image(),
            1845,
            956,
            image::imageops::FilterType::Triangle,
        )
    };
    let (w, h) = (im.width() as usize, im.height() as usize);
    let pixels: Vec<_> = im.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    let image = Image {
        pixels: &pixels,
        width: w,
        height: h,
    };
    let mut desktop = std::env::var_os("PYRA_STATIC_DESKTOP")
        .map(|_| StaticDesktop::new(&image))
        .transpose()?;
    let mut detector = Detector::new(&PathBuf::from(std::env::var("PYRA_RUNTIME")?))?;
    let mut tracker = crate::tracking::Tracker::default();
    let tiles = crate::scanning::grid(w, h, 800);
    let full = Region { x: 0, y: 0, w, h };
    let start = Instant::now();
    let mut reference = None;
    let mut max_shift = 0f32;
    for frame in 0..90 {
        let now = start + std::time::Duration::from_millis(frame * 100);
        let native_frame = desktop
            .as_mut()
            .map(|scene| scene.frame(&tracker))
            .transpose()?;
        let image = native_frame.as_ref().unwrap_or(&image);
        if native_frame.is_some() {
            let altered = image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .filter(|(a, b)| a[..3] != b[..3])
                .count();
            anyhow::ensure!(
                altered == 0,
                "overlay feedback changed {altered} source pixels at frame {frame}"
            );
        }
        tracker.frame(image, now);
        let region = match frame % 3 {
            0 => full,
            1 if !tracker.tracks.is_empty() => {
                crate::worker::face_region(&tracker.tracks[0].face, w, h)
            }
            _ => tiles[(frame as usize / 3) % tiles.len()],
        };
        let found = detector.detect(image, region, 0.65)?;
        tracker.verify(
            &found,
            region,
            if frame % 3 == 0 {
                w.max(h) as f32 / 640. * 24.
            } else {
                0.
            },
        );
        tracker.update(found, now);
        if frame >= 18 {
            anyhow::ensure!(
                tracker.tracks.len() == 1,
                "static image has {} tracks at {frame}",
                tracker.tracks.len()
            );
            let bar = tracker.tracks[0].face.bar(1., 1.);
            let origin = *reference.get_or_insert((bar.x, bar.y));
            max_shift = max_shift.max((bar.x - origin.0).hypot(bar.y - origin.1));
        }
    }
    println!("static image maximum bar displacement={max_shift:.2}px");
    anyhow::ensure!(max_shift < 4., "stationary bar moved {max_shift}px");
    Ok(())
}

fn reference_eyes(scale: f32) -> Vec<[[f32; 2]; 2]> {
    include_str!("../validation/anime-eyes.txt")
        .lines()
        .filter(|s| !s.starts_with('#') && !s.is_empty())
        .map(|line| {
            let v: Vec<f32> = line
                .split_whitespace()
                .map(|v| v.parse::<f32>().unwrap() * scale)
                .collect();
            [[v[0], v[1]], [v[2], v[3]]]
        })
        .collect()
}

fn rotated_case(image: &RgbImage, angle: f32) -> (RgbImage, Vec<[[f32; 2]; 2]>) {
    let mut output = RgbImage::from_pixel(1800, 1800, Rgb([235, 235, 235]));
    let (s, c) = angle.sin_cos();
    for y in 0..1800 {
        for x in 0..1800 {
            let (dx, dy) = (x as f32 - 900., y as f32 - 900.);
            let sx = dx * c + dy * s + 768.;
            let sy = -dx * s + dy * c + 768.;
            if sx >= 0. && sy >= 0. && sx < 1536. && sy < 1536. {
                output.put_pixel(x, y, *image.get_pixel(sx as u32, sy as u32));
            }
        }
    }
    let eyes = reference_eyes(1.)
        .into_iter()
        .map(|pair| {
            pair.map(|e| {
                let (dx, dy) = (e[0] - 768., e[1] - 768.);
                [900. + dx * c - dy * s, 900. + dx * s + dy * c]
            })
        })
        .collect();
    (output, eyes)
}

fn score(faces: &[Face], eyes: &[[[f32; 2]; 2]]) -> (usize, usize, usize) {
    let mut used = vec![false; faces.len()];
    let (mut matched, mut covered) = (0, 0);
    for expected in eyes {
        let cx = (expected[0][0] + expected[1][0]) * 0.5;
        let cy = (expected[0][1] + expected[1][1]) * 0.5;
        let radius = (expected[0][0] - expected[1][0])
            .hypot(expected[0][1] - expected[1][1])
            .max(8.)
            * 1.5;
        let best = faces
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                !used[*i]
                    && cx >= f.x - f.w * 0.1
                    && cx <= f.x + f.w * 1.1
                    && cy >= f.y - f.h * 0.1
                    && cy <= f.y + f.h * 1.1
            })
            .map(|(i, f)| {
                let x = (f.eyes[0][0] + f.eyes[1][0]) * 0.5;
                let y = (f.eyes[0][1] + f.eyes[1][1]) * 0.5;
                (i, (x - cx).hypot(y - cy))
            })
            // A profile has only one marked pupil, while the detector returns
            // the center of the entire visible eye. Match using face scale as
            // well; actual bar coverage is still checked against the pupil.
            .filter(|(i, d)| *d <= radius.max(faces[*i].w.min(faces[*i].h) * 0.4))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            used[i] = true;
            matched += 1;
            let bar = faces[i].bar(1., 1.);
            let (s, c) = bar.angle.sin_cos();
            if expected.iter().all(|eye| {
                let dx = eye[0] - bar.x;
                let dy = eye[1] - bar.y;
                (dx * c + dy * s).abs() <= bar.w * 0.5 && (-dx * s + dy * c).abs() <= bar.h * 0.5
            }) {
                covered += 1;
            }
        }
    }
    (matched, covered, used.iter().filter(|b| !**b).count())
}

fn scan(
    detector: &mut Detector,
    art: &mut crate::stylized::Stylized,
    image: &RgbImage,
    anime: bool,
) -> Result<Vec<Face>> {
    let mut pixels = Vec::with_capacity(image.width() as usize * image.height() as usize * 4);
    for pixel in image.pixels() {
        pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    let image = Image {
        pixels: &pixels,
        width: image.width() as usize,
        height: image.height() as usize,
    };
    let mut regions = vec![Region {
        x: 0,
        y: 0,
        w: image.width,
        h: image.height,
    }];
    if image.width > 800 || image.height > 800 {
        regions.extend(crate::scanning::grid(image.width, image.height, 800));
    }
    let mut found = Vec::new();
    for region in regions {
        if anime {
            let mut styled = art.detect(&image, region, 0.35)?;
            for human in detector.detect_human(&image, region, 0.65)? {
                if styled.iter().all(|f| !f.same_face(&human)) {
                    styled.push(human);
                }
            }
            found.extend(styled);
        } else {
            found.extend(detector.detect_human(&image, region, 0.65)?);
        }
    }
    Ok(suppress(found))
}

fn draw(image: &mut RgbImage, faces: &[Face]) {
    for f in faces {
        let bar = f.bar(1., 1.);
        let (s, c) = bar.angle.sin_cos();
        for y in 0..image.height() {
            for x in 0..image.width() {
                let dx = x as f32 + 0.5 - bar.x;
                let dy = y as f32 + 0.5 - bar.y;
                if (dx * c + dy * s).abs() <= bar.w * 0.5 && (-dx * s + dy * c).abs() <= bar.h * 0.5
                {
                    image.put_pixel(x, y, Rgb([0, 0, 0]));
                }
            }
        }
        for eye in f.eyes {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (x, y) = (eye[0] as i32 + dx, eye[1] as i32 + dy);
                    if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
                        image.put_pixel(x as u32, y as u32, Rgb([255, 60, 60]));
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires PYRA_CORPUS, PYRA_REPORT, PYRA_ART_MODELS and PYRA_RUNTIME"]
fn varied_image_benchmark() -> Result<()> {
    let root = PathBuf::from(std::env::var("PYRA_CORPUS")?);
    let report = PathBuf::from(std::env::var("PYRA_REPORT")?);
    std::fs::create_dir_all(&report)?;
    let group = image::open(root.join("generated-group.jpg"))?.to_rgb8();
    image::imageops::resize(&group, 1536, 1536, image::imageops::FilterType::Triangle)
        .save(report.join("annotation-reference.png"))?;
    let mut detector = Detector::new(&crate::native::runtime())?;
    let mut art = crate::stylized::Stylized::from_directory(&PathBuf::from(std::env::var(
        "PYRA_ART_MODELS",
    )?))?;
    let mut files: Vec<_> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| matches!(p.extension().and_then(|s| s.to_str()), Some("png" | "jpg")))
        .collect();
    files.sort();
    let mut contact =
        RgbImage::from_pixel(960, files.len().div_ceil(3) as u32 * 400, Rgb([48, 48, 48]));
    let mut csv = String::from("file,width,height,yunet_faces,anime_faces,yunet_ms,anime_ms\n");
    for (i, path) in files.iter().enumerate() {
        let image = image::open(path)?.to_rgb8();
        let start = Instant::now();
        let human = scan(&mut detector, &mut art, &image, false)?;
        let ht = start.elapsed().as_millis();
        let start = Instant::now();
        let anime = scan(&mut detector, &mut art, &image, true)?;
        let at = start.elapsed().as_millis();
        if matches!(
            path.file_name().unwrap().to_str(),
            Some("golden-noface.png" | "png_full.png")
        ) {
            anyhow::ensure!(
                anime.is_empty(),
                "false face on no-face control {}",
                path.display()
            );
        }
        println!(
            "{} {}x{}: YuNet={} ({ht}ms), anime={} ({at}ms)",
            path.file_name().unwrap().to_string_lossy(),
            image.width(),
            image.height(),
            human.len(),
            anime.len()
        );
        csv.push_str(&format!(
            "{},{},{},{},{},{ht},{at}\n",
            path.file_name().unwrap().to_string_lossy(),
            image.width(),
            image.height(),
            human.len(),
            anime.len()
        ));
        let thumb = image::imageops::thumbnail(&image, 310, 180);
        image::imageops::replace(
            &mut contact,
            &thumb,
            ((i % 3) * 320) as i64,
            ((i / 3) * 400) as i64,
        );
        let mut result = image.clone();
        draw(&mut result, &anime);
        result.save(report.join(path.file_stem().unwrap()).with_extension("png"))?;
        let thumb = image::imageops::thumbnail(&result, 310, 180);
        image::imageops::replace(
            &mut contact,
            &thumb,
            ((i % 3) * 320) as i64,
            ((i / 3) * 400 + 200) as i64,
        );
    }
    contact.save(report.join("contact.png"))?;
    std::fs::write(report.join("counts.csv"), csv)?;
    let mut metrics = String::from(
        "case,expected,old_matched,old_eye_centers_covered,old_extra,new_matched,new_eye_centers_covered,new_extra\n",
    );
    let mut cases = vec![("original".to_string(), group.clone(), reference_eyes(2.))];
    for side in [1536, 768, 614] {
        let resized =
            image::imageops::resize(&group, side, side, image::imageops::FilterType::Triangle);
        cases.push((
            format!("size-{side}"),
            resized.clone(),
            reference_eyes(side as f32 / 1536.),
        ));
        if side == 1536 {
            for degrees in [-12f32, 12.] {
                let (rotated, eyes) = rotated_case(&resized, degrees.to_radians());
                cases.push((format!("tilt-{degrees}"), rotated, eyes));
            }
            cases.push((
                "dim".into(),
                image::imageops::brighten(&resized, -65),
                reference_eyes(1.),
            ));
            cases.push((
                "blur".into(),
                image::imageops::blur(&resized, 1.2),
                reference_eyes(1.),
            ));
        }
    }
    let mut total_old = 0;
    let mut total_new = 0;
    for (name, image, expected) in cases {
        let before = scan(&mut detector, &mut art, &image, false)?;
        let after = scan(&mut detector, &mut art, &image, true)?;
        let a = score(&before, &expected);
        let b = score(&after, &expected);
        total_old += a.1;
        total_new += b.1;
        let minimum = match name.as_str() {
            "original" => 35,
            "size-1536" => 34,
            "dim" => 33,
            "blur" => 32,
            "size-768" => 24,
            "size-614" => 23,
            _ => 25,
        };
        anyhow::ensure!(
            b.1 >= minimum,
            "eye coverage regressed in {name}: {}/{}, minimum={minimum}",
            b.1,
            expected.len()
        );
        let row = format!(
            "{name},{},{},{},{},{},{},{}\n",
            expected.len(),
            a.0,
            a.1,
            a.2,
            b.0,
            b.1,
            b.2
        );
        print!("{row}");
        metrics.push_str(&row);
    }
    std::fs::write(report.join("eye-coverage.csv"), metrics)?;
    anyhow::ensure!(
        total_new > total_old,
        "candidate did not improve annotated eye-center coverage"
    );
    Ok(())
}

#[test]
#[ignore = "requires PYRA_CORPUS, PYRA_REPORT, and PYRA_RUNTIME"]
fn varied_anime_motion() -> Result<()> {
    use crate::tracking::Tracker;
    use std::time::Duration;
    let root = PathBuf::from(std::env::var("PYRA_CORPUS")?);
    let group = image::open(root.join("generated-group.jpg"))?.to_rgb8();
    let choices = [
        (0, 0, [[101., 51.], [130., 75.]]),
        (2, 1, [[107., 108.], [153., 109.]]),
        (4, 5, [[147., 103.], [147., 103.]]),
    ];
    let portraits: Vec<_> = choices
        .iter()
        .map(|&(x, y, _)| {
            image::imageops::resize(
                &image::imageops::crop_imm(&group, x * 512, y * 512, 512, 512).to_image(),
                256,
                256,
                image::imageops::FilterType::Triangle,
            )
        })
        .collect();
    let mut detector = Detector::new(&crate::native::runtime())?;
    let mut tracker = Tracker::default();
    let start = Instant::now();
    let (mut covered, mut total, mut missing) = (0, 0, 0);
    for frame in 0..90usize {
        let p = frame % 60;
        let triangle = if p <= 30 { p } else { 60 - p } as f32;
        let mut scene = RgbImage::from_pixel(960, 600, Rgb([214, 214, 214]));
        let mut expected = Vec::new();
        for (i, portrait) in portraits.iter().enumerate() {
            let x = 40. + i as f32 * 290. + triangle * (if i == 1 { -1. } else { 1. });
            let y = 90. + triangle * (i as f32 + 1.);
            image::imageops::replace(&mut scene, portrait, x as i64, y as i64);
            expected.push(choices[i].2.map(|e| [e[0] + x, e[1] + y]));
        }
        let mut pixels = Vec::with_capacity(960 * 600 * 4);
        for p in scene.pixels() {
            pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        let image = Image {
            pixels: &pixels,
            width: 960,
            height: 600,
        };
        let now = start + Duration::from_millis(frame as u64 * 50);
        tracker.frame(&image, now);
        if frame % 4 == 0 {
            let found = detector.detect(
                &image,
                Region {
                    x: 0,
                    y: 0,
                    w: 960,
                    h: 600,
                },
                0.65,
            )?;
            tracker.update(found, now);
        }
        let faces: Vec<_> = tracker.tracks.iter().map(|t| t.face.clone()).collect();
        let result = score(&faces, &expected);
        covered += result.1;
        total += expected.len();
        missing += expected.len() - result.0;
    }
    println!(
        "90 motion frames, three different anime faces: eye-center coverage={covered}/{total}, missing face-frames={missing}"
    );
    let report = PathBuf::from(std::env::var("PYRA_REPORT")?);
    std::fs::create_dir_all(&report)?;
    std::fs::write(
        report.join("motion.txt"),
        format!(
            "frames=90\nportraits=3\ncovered_face_frames={covered}\nexpected_face_frames={total}\nmissing_face_frames={missing}\n"
        ),
    )?;
    for (i, portrait) in portraits.iter().enumerate() {
        portrait.save(report.join(format!("motion-portrait-{i}.png")))?;
    }
    anyhow::ensure!(
        covered * 100 >= total * 95,
        "anime motion eye coverage below 95%: {covered}/{total}"
    );
    Ok(())
}

#[test]
#[ignore = "requires PYRA_PHOTOS, PYRA_REPORT, PYRA_ART_MODELS and PYRA_RUNTIME"]
fn annotated_real_groups() -> Result<()> {
    let root = PathBuf::from(std::env::var("PYRA_PHOTOS")?);
    let mut expected = std::collections::BTreeMap::<String, Vec<[[f32; 2]; 2]>>::new();
    for row in include_str!("../validation/annotations.tsv")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let v: Vec<_> = row.split('\t').collect();
        let eyes = [
            [v[5].parse()?, v[6].parse()?],
            [v[7].parse()?, v[8].parse()?],
        ];
        expected.entry(v[0].to_owned()).or_default().push(eyes);
    }
    let mut detector = Detector::new(&crate::native::runtime())?;
    let mut art = crate::stylized::Stylized::from_directory(&PathBuf::from(std::env::var(
        "PYRA_ART_MODELS",
    )?))?;
    let mut csv = String::from(
        "file,expected,old_matched,old_eye_centers_covered,old_extra,new_matched,new_eye_centers_covered,new_extra\n",
    );
    let (mut old_total, mut new_total) = (0, 0);
    for (name, eyes) in expected {
        let image = image::open(root.join(&name))?.to_rgb8();
        let before = scan(&mut detector, &mut art, &image, false)?;
        let after = scan(&mut detector, &mut art, &image, true)?;
        let a = score(&before, &eyes);
        let b = score(&after, &eyes);
        old_total += a.1;
        new_total += b.1;
        let row = format!(
            "{name},{},{},{},{},{},{},{}\n",
            eyes.len(),
            a.0,
            a.1,
            a.2,
            b.0,
            b.1,
            b.2
        );
        print!("{row}");
        csv.push_str(&row);
    }
    let report = PathBuf::from(std::env::var("PYRA_REPORT")?);
    std::fs::create_dir_all(&report)?;
    std::fs::write(report.join("real-groups.csv"), csv)?;
    anyhow::ensure!(
        new_total >= old_total,
        "real-face eye coverage regressed: {old_total} -> {new_total}"
    );
    Ok(())
}

#[test]
#[ignore = "requires PYRA_CORPUS, PYRA_REPORT and PYRA_RUNTIME; six-second live inference test"]
fn asynchronous_anime_motion() -> Result<()> {
    asynchronous_motion(false)
}

#[test]
#[ignore = "requires PYRA_CORPUS, PYRA_REPORT and PYRA_RUNTIME; fast scrolling and reversals"]
fn asynchronous_fast_scrolling() -> Result<()> {
    asynchronous_motion(true)
}

fn asynchronous_motion(fast: bool) -> Result<()> {
    use crate::{
        analysis::{Analyzer, Job},
        capture::Monitor,
        tracking::Tracker,
    };
    use std::time::Duration;
    let root = PathBuf::from(std::env::var("PYRA_CORPUS")?);
    let group = image::open(root.join("generated-group.jpg"))?.to_rgb8();
    let choices = [
        (0, 0, [[101., 51.], [130., 75.]]),
        (2, 1, [[107., 108.], [153., 109.]]),
        (4, 5, [[147., 103.], [147., 103.]]),
    ];
    let portraits: Vec<_> = choices
        .iter()
        .map(|&(x, y, _)| {
            image::imageops::resize(
                &image::imageops::crop_imm(&group, x * 512, y * 512, 512, 512).to_image(),
                256,
                256,
                image::imageops::FilterType::Triangle,
            )
        })
        .collect();
    let analyzer = Analyzer::new()?;
    let mut tracker = Tracker::default();
    let mut inflight = false;
    let (mut covered, mut total, mut jobs, mut max_frame_ms, mut extras) = (0, 0, 0, 0, 0);
    let tiles = crate::scanning::grid(1920, 1080, 800);
    let mut failures_printed = 0;
    let mut desktop: Option<StaticDesktop> = None;
    for frame in 0..120usize {
        let begin = Instant::now();
        let p = frame % 60;
        let triangle = if p <= 30 { p } else { 60 - p } as f32;
        let mut scene = RgbImage::from_pixel(1920, 1080, Rgb([214, 214, 214]));
        let mut expected = Vec::new();
        for (i, portrait) in portraits.iter().enumerate() {
            let x = 190.
                + i as f32 * 320.
                + if fast {
                    0.
                } else {
                    triangle * (if i == 1 { -1. } else { 1. })
                };
            let y = if fast {
                let offsets = [
                    0., 120., 300., 540., 360., 120., 0., 0., 0., 240., 480., 300., 60., 0., 0., 0.,
                ];
                90. + if frame < 40 {
                    0.
                } else {
                    offsets[(frame - 40) % offsets.len()]
                }
            } else {
                170. + triangle * (i as f32 + 1.)
            };
            image::imageops::replace(&mut scene, portrait, x as i64, y as i64);
            expected.push(choices[i].2.map(|e| [e[0] + x, e[1] + y]));
        }
        let mut pixels = Vec::with_capacity(1920 * 1080 * 4);
        for p in scene.pixels() {
            pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        if fast && std::env::var_os("PYRA_SCROLL_DESKTOP").is_some() {
            let source = Image {
                pixels: &pixels,
                width: 1920,
                height: 1080,
            };
            if let Some(desktop) = &mut desktop {
                desktop.present(&source)?;
            } else {
                desktop = Some(StaticDesktop::new(&source)?);
            }
            let captured = desktop.as_mut().unwrap().frame(&tracker)?;
            anyhow::ensure!(
                captured
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(pixels.as_chunks::<4>().0)
                    .all(|(a, b)| a[..3] == b[..3]),
                "scroll capture contains overlay pixels or stale image regions at frame {frame}"
            );
            pixels.copy_from_slice(captured.pixels);
        }
        tracker.frame(
            &Image {
                pixels: &pixels,
                width: 1920,
                height: 1080,
            },
            Instant::now(),
        );
        // Deliberately deliver fast-scroll results only every third capture:
        // projection must correct a stale image as well as frame-to-frame flow.
        if (!fast || frame % 3 == 0)
            && let Some(done) = analyzer.poll()?
        {
            inflight = false;
            jobs += 1;
            if let Some(found) = tracker.project(
                &Image {
                    pixels: &done.job.pixels,
                    width: 1920,
                    height: 1080,
                },
                done.faces,
            ) {
                tracker.update(found, Instant::now());
            }
        }
        if !inflight {
            let region = if jobs % 3 == 1 && !tracker.tracks.is_empty() {
                crate::worker::face_region(
                    &tracker.tracks[(jobs / 3) % tracker.tracks.len()].face,
                    1920,
                    1080,
                )
            } else if jobs % 3 == 2 {
                tiles[(jobs / 3) % tiles.len()]
            } else {
                Region {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                }
            };
            analyzer.submit(Job {
                monitor: Monitor {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                generation: 0,
                pixels,
                region,
                minimum: 0.,
                tile: None,
                threshold: 0.65,
                mode: 0,
                captured: Instant::now(),
            })?;
            inflight = true;
        }
        if frame >= 40 {
            let faces: Vec<_> = tracker.tracks.iter().map(|t| t.face.clone()).collect();
            let result = score(&faces, &expected);
            if (result.1 < 3 || result.2 > 0) && failures_printed < 3 {
                println!("frame {frame}: expected={expected:?}; actual={faces:?}");
                failures_printed += 1;
            }
            covered += result.1;
            extras += result.2;
            total += expected.len();
        }
        max_frame_ms = max_frame_ms.max(begin.elapsed().as_millis());
        if let Some(wait) = Duration::from_millis(50).checked_sub(begin.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    println!(
        "asynchronous 1920x1080 motion fast={fast}: coverage={covered}/{total}; extra face-frames={extras}; completed scans={jobs}; max frame={max_frame_ms}ms"
    );
    let report = PathBuf::from(std::env::var("PYRA_REPORT")?);
    std::fs::create_dir_all(&report)?;
    std::fs::write(
        report.join(if fast {
            "fast-scroll.txt"
        } else {
            "async-motion.txt"
        }),
        format!(
            "frames=120\nwarmup_frames=40\nwidth=1920\nheight=1080\nexpected_face_frames={total}\ncovered_face_frames={covered}\nextra_face_frames={extras}\ncompleted_scans={jobs}\nmaximum_frame_processing_ms={max_frame_ms}\n"
        ),
    )?;
    anyhow::ensure!(
        covered * 100 >= total * 98,
        "asynchronous eye coverage below 98%: {covered}/{total}"
    );
    anyhow::ensure!(
        extras * 100 <= total,
        "duplicate bars survived in asynchronous motion: {extras}"
    );
    Ok(())
}
