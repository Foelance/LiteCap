//! Two 32x32 RGBA icons (idle / recording) built as const arrays at compile
//! time, avoiding a dependency on the `image` crate purely to decode two
//! embedded PNGs.

use tray_icon::Icon;

const SIZE: u32 = 32;

/// Solid dark-gray rounded square with a light border, no dot.
fn build(dot_color: Option<[u8; 4]>) -> Vec<u8> {
    let n = SIZE as i32;
    let center = n / 2;
    let radius = n / 2 - 2;
    let bg = [58u8, 58, 62, 255];
    let border = [110u8, 110, 116, 255];
    let mut buf = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..n {
        for x in 0..n {
            let dx = x - center;
            let dy = y - center;
            let dist_sq = dx * dx + dy * dy;
            let idx = ((y * n + x) * 4) as usize;
            let px = if dist_sq <= radius * radius {
                if dist_sq >= (radius - 2) * (radius - 2) {
                    border
                } else {
                    bg
                }
            } else {
                [0, 0, 0, 0]
            };
            buf[idx..idx + 4].copy_from_slice(&px);
        }
    }

    if let Some(color) = dot_color {
        let dot_radius = radius / 2;
        for y in 0..n {
            for x in 0..n {
                let dx = x - center;
                let dy = y - center;
                if dx * dx + dy * dy <= dot_radius * dot_radius {
                    let idx = ((y * n + x) * 4) as usize;
                    buf[idx..idx + 4].copy_from_slice(&color);
                }
            }
        }
    }

    buf
}

pub fn idle() -> Icon {
    Icon::from_rgba(build(None), SIZE, SIZE).expect("valid idle icon buffer")
}

pub fn recording() -> Icon {
    Icon::from_rgba(build(Some([220, 40, 40, 255])), SIZE, SIZE).expect("valid recording icon buffer")
}
