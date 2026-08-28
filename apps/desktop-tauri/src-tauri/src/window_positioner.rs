// Public positioning API — consumed by the shell and tested here.
#![allow(
    dead_code,
    reason = "window positioner types reserved for future window management integration"
)]

/// A rectangle in physical pixels (monitor work area or icon bounds).
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Panel dimensions in logical pixels.
#[derive(Debug, Clone, Copy)]
pub struct PanelSize {
    pub width: u32,
    pub height: u32,
}

/// Margin kept between the panel edge and the monitor work-area edge.
const MARGIN: i32 = 8;
const GAP: i32 = 8;

fn physical_panel_size(panel_size: &PanelSize, scale_factor: f64) -> (i32, i32) {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };

    // Physical size is rounded to whole pixels before truncation.
    #[expect(clippy::cast_possible_truncation, reason = "whole units by design")]
    let width = ((panel_size.width as f64) * scale_factor).round().max(1.0) as i32;
    #[expect(clippy::cast_possible_truncation, reason = "whole units by design")]
    let height = ((panel_size.height as f64) * scale_factor).round().max(1.0) as i32;
    (width, height)
}

fn clamp_to_work_area(
    target_x: i32,
    target_y: i32,
    monitor_rect: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
) -> (i32, i32) {
    let (pw, ph) = physical_panel_size(panel_size, scale_factor);
    let min_x = monitor_rect.x + MARGIN;
    let min_y = monitor_rect.y + MARGIN;
    // Monitor dimensions are physical pixels, bounded well below i32::MAX.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let max_x = (monitor_rect.x + monitor_rect.width as i32 - pw - MARGIN).max(min_x);
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let max_y = (monitor_rect.y + monitor_rect.height as i32 - ph - MARGIN).max(min_y);

    (target_x.clamp(min_x, max_x), target_y.clamp(min_y, max_y))
}

fn calculate_anchored_position(
    icon_rect: &Rect,
    monitor_rect: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
    anchor_y: i32,
    open_above: bool,
) -> (i32, i32) {
    let (pw, ph) = physical_panel_size(panel_size, scale_factor);
    // Tray icon widths are tens of pixels, far below i32::MAX.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "tray icon pixel dimensions fit in i32"
    )]
    let anchor_x = icon_rect.x + (icon_rect.width as i32) / 2;
    let target_x = anchor_x - pw / 2;
    let target_y = if open_above {
        anchor_y - ph - GAP
    } else {
        anchor_y + GAP
    };

    clamp_to_work_area(target_x, target_y, monitor_rect, panel_size, scale_factor)
}

pub fn clamp_position_to_work_area(
    target_x: i32,
    target_y: i32,
    monitor_rect: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
) -> (i32, i32) {
    clamp_to_work_area(target_x, target_y, monitor_rect, panel_size, scale_factor)
}

/// Calculate panel position anchored to a tray icon rectangle.
///
/// Placement rules:
/// - Horizontally centered on the icon, clamped to the monitor work area.
/// - Left/right taskbars bottom-align the panel to the work area.
/// - If the icon is in the bottom half of the monitor (bottom taskbar), the
///   panel opens *above* the icon. Otherwise it opens *below*.
pub fn calculate_panel_position(
    icon_rect: &Rect,
    monitor_bounds: &Rect,
    work_area: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
) -> (i32, i32) {
    let my = work_area.y;
    // Work-area and icon dimensions are physical pixels, bounded well below i32::MAX.
    #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
    let mh = work_area.height as i32;

    #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
    let icon_cy = icon_rect.y + (icon_rect.height as i32) / 2;
    let monitor_cy = my + mh / 2;

    let open_above = icon_cy > monitor_cy;
    // Icon bounds are physical pixels, bounded well below i32::MAX.
    #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
    let anchor_y = if open_above {
        icon_rect.y
    } else {
        icon_rect.y + icon_rect.height as i32
    };

    let position = calculate_anchored_position(
        icon_rect,
        work_area,
        panel_size,
        scale_factor,
        anchor_y,
        open_above,
    );
    // Monitor and work-area widths are physical pixels, bounded well below i32::MAX.
    #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
    let bounds_right = monitor_bounds.x + monitor_bounds.width as i32;
    #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
    let work_right = work_area.x + work_area.width as i32;

    if work_area.x > monitor_bounds.x || work_right < bounds_right {
        let (_, ph) = physical_panel_size(panel_size, scale_factor);
        // Work-area height is a physical pixel count, bounded well below i32::MAX.
        #[expect(clippy::cast_possible_wrap, reason = "pixel dimensions fit in i32")]
        let work_height = work_area.height as i32;
        (position.0, work_area.y + work_height - ph - MARGIN)
    } else {
        position
    }
}

/// Position for shortcut-triggered opening: 22 % from left, vertically centred.
pub fn calculate_shortcut_position(
    monitor_rect: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
) -> (i32, i32) {
    let (pw, ph) = physical_panel_size(panel_size, scale_factor);
    let mx = monitor_rect.x;
    let my = monitor_rect.y;
    // Monitor dimensions are physical pixels, bounded well below i32::MAX.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let mw = monitor_rect.width as i32;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let mh = monitor_rect.height as i32;

    // Shortcut offset is truncated to a whole pixel by design.
    #[expect(clippy::cast_possible_truncation, reason = "whole units by design")]
    let x = mx + ((mw as f64) * 0.22) as i32;
    let y = my + (mh - ph) / 2;

    let x = x.max(mx + MARGIN).min(mx + mw - pw - MARGIN);
    let y = y.max(my + MARGIN).min(my + mh - ph - MARGIN);

    (x, y)
}

/// Calculate detached popout/settings placement.
///
/// Placement rules:
/// - With a known tray anchor, centre horizontally on the tray parity anchor
///   point (icon centre-x, top-y) and pick above or below based on available
///   space before clamping inside the work area.
/// - Without a tray anchor, fall back to the bottom-right corner of the work
///   area and clamp so the window remains visible.
pub fn calculate_popout_position(
    icon_rect: Option<&Rect>,
    monitor_rect: &Rect,
    panel_size: &PanelSize,
    scale_factor: f64,
) -> (i32, i32) {
    let (pw, ph) = physical_panel_size(panel_size, scale_factor);
    let mx = monitor_rect.x;
    let my = monitor_rect.y;
    // Monitor dimensions are physical pixels, bounded well below i32::MAX.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let mw = monitor_rect.width as i32;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monitor pixel dimensions fit in i32"
    )]
    let mh = monitor_rect.height as i32;

    let (target_x, target_y) = if let Some(icon_rect) = icon_rect {
        let space_above = icon_rect.y - my - MARGIN;
        let space_below = my + mh - icon_rect.y - MARGIN;
        let open_above = space_above >= ph + GAP || space_above > space_below;
        calculate_anchored_position(
            icon_rect,
            monitor_rect,
            panel_size,
            scale_factor,
            icon_rect.y,
            open_above,
        )
    } else {
        (mx + mw - pw - MARGIN, my + mh - ph - MARGIN)
    };

    clamp_position_to_work_area(target_x, target_y, monitor_rect, panel_size, scale_factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard_monitor() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 2400,
            height: 1080,
        }
    }

    fn hd_monitor() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    fn panel() -> PanelSize {
        PanelSize {
            width: 420,
            height: 560,
        }
    }

    fn tall_monitor() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1400,
        }
    }

    // --- tray-anchor tests ---

    #[test]
    fn bottom_taskbar_panel_opens_above() {
        let icon = Rect {
            x: 1800,
            y: 1040,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (_, y) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(y < icon.y, "panel should sit above the icon");
    }

    #[test]
    fn top_taskbar_panel_opens_below() {
        let icon = Rect {
            x: 900,
            y: 4,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (_, y) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(
            y >= icon.y + i32::try_from(icon.height).unwrap(),
            "panel should sit below the icon"
        );
    }

    #[test]
    fn horizontal_centre_on_icon() {
        let icon = Rect {
            x: 960,
            y: 1040,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (x, _) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        let icon_cx = icon.x + 12;
        let panel_cx = x + 210;
        assert!(
            (icon_cx - panel_cx).abs() <= 1,
            "panel should be centred on icon (off by {})",
            (icon_cx - panel_cx).abs()
        );
    }

    #[test]
    fn clamped_left_edge() {
        let icon = Rect {
            x: 0,
            y: 1040,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (x, _) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(x >= MARGIN, "panel must not exceed left margin");
    }

    #[test]
    fn clamped_right_edge() {
        let icon = Rect {
            x: 1900,
            y: 1040,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (x, _) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(
            x + i32::try_from(panel().width).unwrap() + MARGIN <= 1920,
            "panel must not exceed right margin"
        );
    }

    #[test]
    fn clamped_top_edge() {
        let icon = Rect {
            x: 960,
            y: 4,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (_, y) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(y >= MARGIN, "panel must not exceed top margin");
    }

    #[test]
    fn top_taskbar_work_area_clamps_open_below_to_min_y() {
        let work_area = Rect {
            x: 0,
            y: 40,
            width: 1920,
            height: 1040,
        };
        let icon = Rect {
            x: 960,
            y: 4,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (_, y) = calculate_panel_position(&icon, &monitor, &work_area, &panel(), 1.0);
        assert_eq!(y, work_area.y + MARGIN);
    }

    #[test]
    fn left_taskbar_bottom_aligns_panel() {
        let monitor = hd_monitor();
        let work_area = Rect {
            x: 40,
            y: 0,
            width: 1880,
            height: 1080,
        };
        let icon = Rect {
            x: 8,
            y: 1048,
            width: 24,
            height: 24,
        };

        let (_, y) = calculate_panel_position(&icon, &monitor, &work_area, &panel(), 1.0);

        assert_eq!(y, 1080 - 560 - MARGIN);
    }

    #[test]
    fn high_dpi_right_taskbar_bottom_aligns_panel() {
        let monitor = Rect {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        let work_area = Rect {
            x: 0,
            y: 0,
            width: 3760,
            height: 2160,
        };
        let icon = Rect {
            x: 3808,
            y: 2128,
            width: 24,
            height: 24,
        };

        let (_, y) = calculate_panel_position(&icon, &monitor, &work_area, &panel(), 2.0);

        assert_eq!(y, 2160 - (560 * 2) - MARGIN);
    }

    #[test]
    fn multi_monitor_offset() {
        let monitor = Rect {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let icon = Rect {
            x: 3700,
            y: 1040,
            width: 24,
            height: 24,
        };
        let (x, _) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        assert!(x >= monitor.x + MARGIN);
        assert!(
            x + i32::try_from(panel().width).unwrap() + MARGIN
                <= monitor.x + i32::try_from(monitor.width).unwrap()
        );
    }

    #[test]
    fn high_dpi_positioning() {
        let icon = Rect {
            x: 960,
            y: 1040,
            width: 24,
            height: 24,
        };
        let monitor = hd_monitor();
        let (x1, y1) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        let (x2, y2) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 2.0);
        assert!(
            x2 < x1,
            "higher scale should shift the panel left to fit its physical width"
        );
        assert!(
            y2 < y1,
            "higher scale should shift the panel upward to fit its physical height"
        );
    }

    // --- shortcut-anchor tests ---

    #[test]
    fn shortcut_position_22_pct_from_left() {
        let monitor = hd_monitor();
        let (x, _) = calculate_shortcut_position(&monitor, &panel(), 1.0);
        // 1920 * 0.22 = 422.4 — a constant that fits i32.
        #[expect(clippy::cast_possible_truncation, reason = "constant offset fits i32")]
        let expected_x = (1920.0 * 0.22) as i32;
        assert_eq!(x, expected_x);
    }

    #[test]
    fn shortcut_position_vertically_centred() {
        let (_, y) = calculate_shortcut_position(&hd_monitor(), &panel(), 1.0);
        let expected_y = (1080 - 560) / 2;
        assert_eq!(y, expected_y);
    }

    #[test]
    fn shortcut_clamped_small_monitor() {
        let monitor = Rect {
            x: 0,
            y: 0,
            width: 500,
            height: 600,
        };
        let (x, y) = calculate_shortcut_position(&monitor, &panel(), 1.0);
        assert!(x >= MARGIN);
        assert!(
            x + i32::try_from(panel().width).unwrap() + MARGIN
                <= i32::try_from(monitor.width).unwrap()
        );
        assert!(y >= MARGIN);
        assert!(
            y + i32::try_from(panel().height).unwrap() + MARGIN
                <= i32::try_from(monitor.height).unwrap()
        );
    }

    // --- visible-surface popout tests ---

    #[test]
    fn anchored_popout_keeps_no_anchor_bottom_right_fallback() {
        let work_area = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let target = calculate_popout_position(None, &work_area, &panel(), 1.0);
        assert_eq!(target, (1492, 512));
    }

    #[test]
    fn anchored_popout_uses_same_tray_anchor_x_as_panel_positioning() {
        let icon = Rect {
            x: 1800,
            y: 1040,
            width: 24,
            height: 24,
        };

        let monitor = standard_monitor();
        let (panel_x, _) = calculate_panel_position(&icon, &monitor, &monitor, &panel(), 1.0);
        let (popout_x, _) =
            calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 1.0);

        assert_eq!(popout_x, panel_x);
    }

    #[test]
    fn popout_clamps_inside_monitor_bounds() {
        let icon = Rect {
            x: 1910,
            y: 1040,
            width: 24,
            height: 24,
        };
        let (x, y) = calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 1.0);
        assert!(x >= 8);
        assert!(y >= 8);
    }

    #[test]
    fn tray_anchored_popout_centres_on_known_anchor() {
        let icon = Rect {
            x: 1800,
            y: 1040,
            width: 24,
            height: 24,
        };
        let (x, _) = calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 1.0);
        let icon_cx = icon.x + 12;
        let panel_cx = x + 210;
        assert!((icon_cx - panel_cx).abs() <= 1);
    }

    #[test]
    fn popout_prefers_above_tray_when_space_below_is_tight() {
        let icon = Rect {
            x: 1600,
            y: 1040,
            width: 24,
            height: 24,
        };
        let (_, y) = calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 1.0);
        assert!(y < icon.y);
    }

    #[test]
    fn popout_prefers_below_tray_when_space_above_is_tight() {
        let icon = Rect {
            x: 1600,
            y: 8,
            width: 24,
            height: 24,
        };
        let (_, y) = calculate_popout_position(Some(&icon), &tall_monitor(), &panel(), 1.0);
        assert!(y > icon.y);
    }

    #[test]
    fn top_taskbar_popout_uses_tray_top_anchor_point() {
        let icon = Rect {
            x: 1600,
            y: 8,
            width: 24,
            height: 24,
        };
        let (_, y) = calculate_popout_position(Some(&icon), &tall_monitor(), &panel(), 1.0);
        assert_eq!(y, icon.y + GAP);
    }

    #[test]
    fn popout_high_dpi_uses_physical_panel_size() {
        let icon = Rect {
            x: 1800,
            y: 1040,
            width: 24,
            height: 24,
        };
        let (x1, y1) = calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 1.0);
        let (x2, y2) = calculate_popout_position(Some(&icon), &standard_monitor(), &panel(), 2.0);

        assert!(
            x2 < x1,
            "higher scale should account for wider physical popout bounds"
        );
        assert!(
            y2 < y1,
            "higher scale should account for taller physical popout bounds"
        );
    }
}
