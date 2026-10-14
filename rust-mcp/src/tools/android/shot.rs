//! Screenshot handling for the touch commands: PNG decode, downscale, grid and crosshair overlay, encode, base64,
//! and the list of images an MCP call hands back as image content blocks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Set by the MCP server loop: images are then returned as content blocks and the file path stays out of the text.
pub static MCP_MODE: AtomicBool = AtomicBool::new(false);
static IMAGES: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

pub fn is_mcp() -> bool {
    MCP_MODE.load(Ordering::Relaxed)
}

/// PNG bytes queued by the last tool call (MCP attaches them to the response).
pub fn queue_image(png: Vec<u8>) {
    if let Ok(mut v) = IMAGES.lock() {
        v.push(png);
    }
}

pub fn take_images() -> Vec<Vec<u8>> {
    IMAGES.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

pub struct Image {
    pub w: usize,
    pub h: usize,
    /// RGB8, row-major.
    pub rgb: Vec<u8>,
}

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
}

pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("not a readable PNG: {}", e))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG decode failed: {}", e))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let data = &buf[..info.buffer_size()];
    let mut rgb = Vec::with_capacity(w * h * 3);
    match info.color_type {
        png::ColorType::Rgb => rgb.extend_from_slice(data),
        png::ColorType::Rgba => data.chunks_exact(4).for_each(|p| rgb.extend_from_slice(&p[..3])),
        png::ColorType::Grayscale => data.iter().for_each(|&g| rgb.extend_from_slice(&[g, g, g])),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).for_each(|p| rgb.extend_from_slice(&[p[0], p[0], p[0]])),
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".to_string()),
    }
    if rgb.len() != w * h * 3 {
        return Err("PNG size does not match its header".to_string());
    }
    Ok(Image { w, h, rgb })
}

pub fn encode(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.w as u32, img.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&img.rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// A cheap fingerprint of what is on screen, ignoring the status bar (its clock changes every minute).
pub fn fingerprint(img: &Image) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let start = img.h / 25;
    for y in (start..img.h).step_by(3) {
        for x in (0..img.w).step_by(3) {
            let i = (y * img.w + x) * 3;
            // Drop the low bits: animation anti-aliasing and video noise are not a change.
            for c in 0..3 {
                h ^= (img.rgb[i + c] >> 3) as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

/// A screenshot of a FLAG_SECURE window (or a screen that is off) comes back entirely black.
pub fn is_blank(img: &Image) -> bool {
    let step = (img.rgb.len() / 3 / 4000).max(1) * 3;
    img.rgb.iter().step_by(step).all(|&b| b < 6)
}

/// Area-average downscale to `nw` x `nh`.
pub fn resize(img: &Image, nw: usize, nh: usize) -> Image {
    if nw >= img.w && nh >= img.h {
        return Image { w: img.w, h: img.h, rgb: img.rgb.clone() };
    }
    let (nw, nh) = (nw.max(1), nh.max(1));
    let mut rgb = vec![0u8; nw * nh * 3];
    for dy in 0..nh {
        let y0 = dy * img.h / nh;
        let y1 = ((dy + 1) * img.h / nh).max(y0 + 1).min(img.h);
        for dx in 0..nw {
            let x0 = dx * img.w / nw;
            let x1 = ((dx + 1) * img.w / nw).max(x0 + 1).min(img.w);
            let mut sum = [0u32; 3];
            for y in y0..y1 {
                let row = y * img.w * 3;
                for x in x0..x1 {
                    let i = row + x * 3;
                    sum[0] += img.rgb[i] as u32;
                    sum[1] += img.rgb[i + 1] as u32;
                    sum[2] += img.rgb[i + 2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (dy * nw + dx) * 3;
            for c in 0..3 {
                rgb[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Image { w: nw, h: nh, rgb }
}

fn blend(img: &mut Image, x: i64, y: i64, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x as usize >= img.w || y as usize >= img.h {
        return;
    }
    let i = (y as usize * img.w + x as usize) * 3;
    for c in 0..3 {
        img.rgb[i + c] = (img.rgb[i + c] as f32 * (1.0 - alpha) + color[c] as f32 * alpha) as u8;
    }
}

const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// A number in a 3x5 pixel font (`scale` times larger) on a dark backing, top-left at (x, y).
fn label(img: &mut Image, x: i64, y: i64, n: i64, scale: i64) {
    let text = n.to_string();
    let (w, h) = (text.len() as i64 * 4 * scale + scale, 6 * scale + scale);
    for yy in 0..h {
        for xx in 0..w {
            blend(img, x + xx, y + yy, [0, 0, 0], 0.65);
        }
    }
    for (k, ch) in text.chars().enumerate() {
        let d = ch.to_digit(10).unwrap_or(0) as usize;
        for (row, bits) in DIGITS[d].iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = x + scale + (k as i64 * 4 + col as i64) * scale + sx;
                            let py = y + scale + row as i64 * scale + sy;
                            blend(img, px, py, [255, 255, 255], 1.0);
                        }
                    }
                }
            }
        }
    }
}

/// A faint grid every 100 device pixels, labelled in device pixels, plus a red crosshair at `mark` (device pixels).
/// `factor` is device pixels per image pixel.
pub fn overlay(img: &mut Image, dev_w: i64, dev_h: i64, factor: f64, mark: Option<(i64, i64)>) {
    let cyan = [0, 200, 255];
    let mut g = 100;
    while g < dev_w {
        let ix = (g as f64 / factor).round() as i64;
        for y in 0..img.h as i64 {
            blend(img, ix, y, cyan, 0.30);
        }
        label(img, ix + 2, 2, g, 2);
        g += 100;
    }
    let mut g = 100;
    while g < dev_h {
        let iy = (g as f64 / factor).round() as i64;
        for x in 0..img.w as i64 {
            blend(img, x, iy, cyan, 0.30);
        }
        label(img, 2, iy + 2, g, 2);
        g += 100;
    }
    if let Some((mx, my)) = mark {
        let (cx, cy) = ((mx as f64 / factor).round() as i64, (my as f64 / factor).round() as i64);
        let red = [255, 0, 0];
        for d in -28..=28i64 {
            if d.abs() > 5 {
                for t in -1..=1 {
                    blend(img, cx + d, cy + t, red, 1.0);
                    blend(img, cx + t, cy + d, red, 1.0);
                }
            }
        }
        for d in -9..=9i64 {
            for (ex, ey) in [(d, -9), (d, 9), (-9, d), (9, d)] {
                blend(img, cx + ex, cy + ey, red, 1.0);
            }
        }
    }
}

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn png_round_trip_resize_and_overlay() {
        let img = Image { w: 40, h: 80, rgb: vec![200; 40 * 80 * 3] };
        let png = encode(&img).unwrap();
        assert!(is_png(&png));
        let back = decode(&png).unwrap();
        assert_eq!((back.w, back.h), (40, 80));
        assert!(!is_blank(&back));
        let mut small = resize(&back, 20, 40);
        assert_eq!((small.w, small.h), (20, 40));
        overlay(&mut small, 40, 80, 2.0, Some((20, 40)));
        assert!(small.rgb.chunks(3).any(|p| p == [255, 0, 0]), "crosshair is drawn");
        assert!(is_blank(&Image { w: 10, h: 10, rgb: vec![0; 300] }));
    }
}
