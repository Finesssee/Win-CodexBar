//! Pixel-level tray icon renderer, decoupled from any platform icon API.
//!
//! Returns raw RGBA bytes so callers (egui tray manager, Tauri shell, tests)
//! can adapt the result to their own icon type without pulling in extra deps.

use image::{ImageBuffer, Rgba, RgbaImage};

use super::icon::UsageLevel;

/// Side length of the generated tray icon in pixels.
pub const TRAY_ICON_SIZE: u32 = 32;

/// Render a usage-bar tray icon as raw RGBA bytes.
///
/// - `session_percent`: primary bar fill (0–100), colour-coded by [`UsageLevel`]
/// - `weekly_percent`: optional secondary bar fill (0–100). When `Some`, two thin
///   bars are drawn (session top, weekly bottom). When `None`, a single thick bar
///   is drawn instead.
/// - `has_error`: desaturate all bar colours to grey to signal an error/unknown state.
///
/// Returns `(rgba_bytes, width, height)` for a [`TRAY_ICON_SIZE`]×[`TRAY_ICON_SIZE`] icon.
pub fn render_bar_icon_rgba(
    session_percent: f64,
    weekly_percent: Option<f64>,
    has_error: bool,
) -> (Vec<u8>, u32, u32) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let mut img: RgbaImage = ImageBuffer::new(SZ, SZ);

    for pixel in img.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }

    let bg_alpha: u8 = if has_error { 180 } else { 255 };
    let bg_color = Rgba([60, 60, 70, bg_alpha]);
    for y in 2..SZ - 2 {
        for x in 2..SZ - 2 {
            img.put_pixel(x, y, bg_color);
        }
    }

    let color_for = |percent: f64| -> (u8, u8, u8) {
        let (r, g, b) = UsageLevel::from_percent(percent).color();
        if has_error {
            // Average of three u8 colour channels: sum ≤ 765, so /3 ≤ 255 fits u8.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "mean of three u8 channels; r+g+b ≤ 765, divided by 3 is ≤ 255 and fits u8"
            )]
            let gray = ((r as u16 + g as u16 + b as u16) / 3) as u8;
            (gray, gray, gray)
        } else {
            (r, g, b)
        }
    };

    let bar_left = 4u32;
    let bar_right = SZ - 4;
    let bar_width = bar_right - bar_left;

    // pct is clamped to 0–100, scaled by bar_width (≤ SZ = 32), so the result fits u32.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "pct clamped to 0–100 and scaled by bar_width ≤ 32; result is a small pixel count that fits u32"
    )]
    let fill_px = |pct: f64| ((pct.clamp(0.0, 100.0) / 100.0) * bar_width as f64) as u32;

    let mut draw_bar = |y_start: u32, y_end: u32, pct: f64| {
        let (r, g, b) = color_for(pct);
        let fill_end = (bar_left + fill_px(pct)).min(bar_right);
        for y in y_start..y_end {
            for x in bar_left..bar_right {
                img.put_pixel(x, y, Rgba([80, 80, 90, 255]));
            }
        }
        for y in y_start..y_end {
            for x in bar_left..fill_end {
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    };

    match weekly_percent {
        Some(weekly) => {
            draw_bar(8, 15, session_percent); // session bar (top, thicker)
            draw_bar(18, 23, weekly); // weekly bar (bottom, thinner)
        }
        None => {
            draw_bar(10, 22, session_percent); // single thick bar (centred)
        }
    }

    (img.into_raw(), SZ, SZ)
}

/// Render a compact numeric percent tray icon as raw RGBA bytes.
pub fn render_percent_icon_rgba(percent: f64, has_error: bool) -> (Vec<u8>, u32, u32) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let mut img: RgbaImage = ImageBuffer::new(SZ, SZ);

    for pixel in img.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }

    let bg_alpha: u8 = if has_error { 180 } else { 255 };
    for y in 2..SZ - 2 {
        for x in 2..SZ - 2 {
            img.put_pixel(x, y, Rgba([60, 60, 70, bg_alpha]));
        }
    }

    // percent clamped to 0–100 before rounding, so the cast to u32 cannot truncate.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "percent is clamped to 0–100 and rounded; the integer fits u32"
    )]
    let pct = percent.clamp(0.0, 100.0).round() as u32;
    let text = if pct >= 100 {
        "100".to_string()
    } else {
        format!("{pct}%")
    };
    let glyph_width = 3u32;
    let glyph_gap = 1u32;
    let scale = if text.len() >= 3 { 2u32 } else { 3u32 };
    // text is "100" or at most "NN%", so its length is ≤ 4 and fits u32.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "text is \"100\" or \"NN%\", so len ≤ 4 and fits u32"
    )]
    let text_len = text.len() as u32;
    let text_width = text_len * glyph_width * scale + (text_len - 1) * glyph_gap;
    let text_height = 5 * scale;
    let start_x = (SZ.saturating_sub(text_width)) / 2;
    let start_y = (SZ.saturating_sub(text_height)) / 2;

    let (r, g, b) = UsageLevel::from_percent(percent).color();
    let color = if has_error {
        // Average of three u8 colour channels: sum ≤ 765, so /3 ≤ 255 fits u8.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "mean of three u8 channels; r+g+b ≤ 765, divided by 3 is ≤ 255 and fits u8"
        )]
        let gray = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        Rgba([gray, gray, gray, 255])
    } else {
        Rgba([r, g, b, 255])
    };

    let mut x = start_x;
    for ch in text.chars() {
        draw_glyph(&mut img, ch, x, start_y, scale, color);
        x += glyph_width * scale + glyph_gap;
    }

    (img.into_raw(), SZ, SZ)
}

fn draw_glyph(img: &mut RgbaImage, ch: char, x: u32, y: u32, scale: u32, color: Rgba<u8>) {
    let Some(rows) = glyph_rows(ch) else {
        return;
    };
    for (row_idx, row) in rows.iter().enumerate() {
        for col in 0..3 {
            let bit = 1 << (2 - col);
            if row & bit == 0 {
                continue;
            }
            for yy in 0..scale {
                for xx in 0..scale {
                    let px = x + col * scale + xx;
                    // row_idx is bounded by the 5-row glyph array, so it fits u32.
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "row_idx iterates over a fixed [u8; 5] glyph, so it is 0..5 and fits u32"
                    )]
                    let py = y + row_idx as u32 * scale + yy;
                    if px < TRAY_ICON_SIZE && py < TRAY_ICON_SIZE {
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }
}

fn glyph_rows(ch: char) -> Option<[u8; 5]> {
    Some(match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_correct_dimensions() {
        let (rgba, w, h) = render_bar_icon_rgba(50.0, None, false);
        assert_eq!(w, TRAY_ICON_SIZE);
        assert_eq!(h, TRAY_ICON_SIZE);
        assert_eq!(u32::try_from(rgba.len()).unwrap(), w * h * 4);
    }

    #[test]
    fn render_two_bar_has_correct_size() {
        let (rgba, w, h) = render_bar_icon_rgba(30.0, Some(60.0), false);
        assert_eq!(u32::try_from(rgba.len()).unwrap(), w * h * 4);
    }

    #[test]
    fn zero_fill_gives_gray_only_bar() {
        let (rgba, w, _h) = render_bar_icon_rgba(0.0, None, false);
        // Sample a pixel near the centre of the bar track area (y=16, x=8)
        let idx = ((16 * w + 8) * 4) as usize;
        // Should be the gray track colour, not a usage colour
        assert_eq!(rgba[idx], 80); // R
        assert_eq!(rgba[idx + 1], 80); // G
        assert_eq!(rgba[idx + 2], 90); // B
    }

    #[test]
    fn full_fill_gives_colored_bar() {
        let (rgba, w, _h) = render_bar_icon_rgba(100.0, None, false);
        // At 100% used the bar is at Critical level
        let idx = ((16 * w + 8) * 4) as usize;
        let (er, eg, eb) = UsageLevel::Critical.color();
        assert_eq!(rgba[idx], er);
        assert_eq!(rgba[idx + 1], eg);
        assert_eq!(rgba[idx + 2], eb);
    }

    #[test]
    fn error_state_desaturates_colors() {
        let (normal, _, _) = render_bar_icon_rgba(100.0, None, false);
        let (error, _, _) = render_bar_icon_rgba(100.0, None, true);
        // In error mode all three channels at the filled bar pixel should be equal (grey)
        let idx = ((16 * 32 + 8) * 4) as usize;
        assert_ne!(normal[idx], normal[idx + 1]); // colour has distinct channels
        assert_eq!(error[idx], error[idx + 1]); // grey: R == G
        assert_eq!(error[idx + 1], error[idx + 2]); // grey: G == B
    }

    #[test]
    fn percent_icon_produces_correct_dimensions() {
        let (rgba, w, h) = render_percent_icon_rgba(72.0, false);
        assert_eq!(w, TRAY_ICON_SIZE);
        assert_eq!(h, TRAY_ICON_SIZE);
        assert_eq!(u32::try_from(rgba.len()).unwrap(), w * h * 4);
    }

    #[test]
    fn percent_icon_draws_visible_text() {
        let (rgba, _, _) = render_percent_icon_rgba(72.0, false);
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[3] == 255 && px[0] != 60)
        );
    }

    #[test]
    fn percent_icon_clamps_to_hundred() {
        let (rgba, w, h) = render_percent_icon_rgba(125.0, false);
        assert_eq!(u32::try_from(rgba.len()).unwrap(), w * h * 4);
    }
}
