//! Persistent per-region dirtiness. One changed frame remains pending until
//! actually scanned; a clock animation does not restart the whole desktop sweep.
use crate::detection::{Image, Region};

struct Tile {
    region: Region,
    fingerprint: Option<u64>,
    dirty: bool,
    scanned: u64,
}
pub struct Scanner {
    tiles: Vec<Tile>,
    cursor: usize,
}

pub fn grid(width: usize, height: usize, side: usize) -> Vec<Region> {
    let axis = |length: usize| {
        let step = side * 3 / 4;
        let count = length.saturating_sub(side).div_ceil(step);
        (0..=count)
            .map(|i| {
                (i * length.saturating_sub(side))
                    .checked_div(count)
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>()
    };
    axis(height)
        .into_iter()
        .flat_map(|y| {
            axis(width).into_iter().map(move |x| Region {
                x,
                y,
                w: side.min(width - x),
                h: side.min(height - y),
            })
        })
        .collect()
}

impl Scanner {
    pub fn new(width: usize, height: usize, mode: u32) -> Self {
        let side = match mode {
            1 => 640,
            2 => 1280,
            _ => 800,
        };
        Self {
            tiles: grid(width, height, side)
                .into_iter()
                .map(|region| Tile {
                    region,
                    fingerprint: None,
                    dirty: true,
                    scanned: 0,
                })
                .collect(),
            cursor: 0,
        }
    }
    pub fn observe(&mut self, image: &Image<'_>, now: u64) -> bool {
        let mut changed = false;
        for tile in &mut self.tiles {
            let hash = fingerprint(image, tile.region);
            if tile.fingerprint != Some(hash) {
                tile.dirty = true;
                changed = true;
            }
            // Verification of stable tiles is infrequent, but never disabled.
            if now.saturating_sub(tile.scanned) >= 5000 {
                tile.dirty = true;
            }
            tile.fingerprint = Some(hash);
        }
        changed
    }
    pub fn next(&self) -> Option<(usize, Region)> {
        self.tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.dirty)
            .min_by_key(|(i, t)| {
                (
                    t.scanned,
                    (i + self.tiles.len() - self.cursor) % self.tiles.len(),
                )
            })
            .map(|(i, t)| (i, t.region))
    }
    pub fn scanned(&mut self, index: usize, now: u64) {
        self.tiles[index].dirty = false;
        self.tiles[index].scanned = now.max(1);
        self.cursor = (index + 1) % self.tiles.len();
    }
}

fn fingerprint(image: &Image<'_>, r: Region) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for y in 0..32 {
        for x in 0..32 {
            let px = r.x + ((2 * x + 1) * r.w / 64).min(r.w - 1);
            let py = r.y + ((2 * y + 1) * r.h / 64).min(r.h - 1);
            let i = (py * image.width + px) * 4;
            for c in 0..3 {
                hash ^= image.pixels[i + c] as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_single_changed_frame_stays_pending_until_scanned() {
        let mut scanner = Scanner::new(1600, 800, 0);
        let mut pixels = vec![255; 1600 * 800 * 4];
        scanner.observe(
            &Image {
                pixels: &pixels,
                width: 1600,
                height: 800,
            },
            100,
        );
        while let Some((i, _)) = scanner.next() {
            scanner.scanned(i, 100);
        }
        for y in 50..200 {
            for x in 1200..1400 {
                pixels[(y * 1600 + x) * 4] = 0;
            }
        }
        scanner.observe(
            &Image {
                pixels: &pixels,
                width: 1600,
                height: 800,
            },
            150,
        );
        let pending = scanner.next().unwrap().0;
        scanner.observe(
            &Image {
                pixels: &pixels,
                width: 1600,
                height: 800,
            },
            200,
        );
        assert_eq!(scanner.next().unwrap().0, pending);
        assert!(scanner.next().unwrap().1.x > 0);
    }
    #[test]
    fn changed_regions_do_not_starve_other_regions() {
        let mut scanner = Scanner::new(5120, 1440, 0);
        let mut pixels = vec![240; 5120 * 1440 * 4];
        let count = scanner.tiles.len();
        let mut visited = std::collections::HashSet::new();
        for frame in 0..count * 2 {
            pixels[100 * 5120 * 4..120 * 5120 * 4].fill(frame as u8);
            scanner.observe(
                &Image {
                    pixels: &pixels,
                    width: 5120,
                    height: 1440,
                },
                frame as u64 + 1,
            );
            if let Some((i, _)) = scanner.next() {
                visited.insert(i);
                scanner.scanned(i, frame as u64 + 1);
            }
        }
        assert_eq!(visited.len(), count);
    }
    #[test]
    fn ultrawide_portrait_and_thin_displays_have_no_scan_gaps() {
        for (w, h) in [
            (3440, 1440),
            (5120, 1440),
            (7680, 2160),
            (11520, 1080),
            (1080, 3840),
            (400, 3000),
            (8192, 512),
            (32, 4096),
        ] {
            for side in [640, 800, 1280] {
                let regions = grid(w, h, side);
                for y in (0..h).step_by(37) {
                    for x in (0..w).step_by(43) {
                        assert!(
                            regions
                                .iter()
                                .any(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
                        );
                    }
                }
                assert!(regions.iter().all(|r| r.w <= side && r.h <= side));
            }
        }
    }
}
