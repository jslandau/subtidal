//! Platform-neutral CC tray icon renderer (signed-distance-field, anti-aliased).
//!
//! Outputs RGBA bytes (R, G, B, A order) at 64×64. Each platform converts to
//! whatever format its tray API needs (ksni ARGB on Linux, NSBitmapImageRep on macOS).

/// Render the CC tray icon into a 64×64 RGBA buffer.
///
/// - `(r, g, b)`: icon color (e.g. (80, 200, 80) for green, (255, 255, 255) for white)
/// - `opacity`: 0.0–1.0 alpha scale applied to every pixel
/// - `strikethrough`: draw a diagonal line across the icon (off-state indicator)
pub fn render_cc_icon_rgba(r: u8, g: u8, b: u8, opacity: f32, strikethrough: bool) -> Vec<u8> {
    const S: i32 = 64;
    const SF: f32 = S as f32;
    // Four bytes per pixel: R, G, B, A.
    let mut data = vec![0u8; (S * S * 4) as usize];

    let blend = |data: &mut Vec<u8>, x: i32, y: i32, coverage: f32, op: f32| {
        if x < 0 || x >= S || y < 0 || y >= S {
            return;
        }
        let idx = ((y * S + x) * 4) as usize;
        let a = (coverage.clamp(0.0, 1.0) * op * 255.0) as u8;
        // Composite: max alpha wins (all shapes share the same color).
        if a > data[idx + 3] {
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = a;
        }
    };

    // --- Rounded rectangle frame ---
    let rect_top = 4.0_f32;
    let rect_bot = 60.0_f32;
    let rect_left = 0.0_f32;
    let rect_right = SF;
    let r_outer = 12.0_f32;
    let stroke = 5.0_f32;
    let r_inner = r_outer - stroke;

    for y in 0..S {
        for x in 0..S {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let sdf_rr = |left: f32, top: f32, right: f32, bot: f32, rad: f32| -> f32 {
                let cx = px.clamp(left + rad, right - rad);
                let cy = py.clamp(top + rad, bot - rad);
                let dx = (px - cx).abs();
                let dy = (py - cy).abs();
                (dx * dx + dy * dy).sqrt() - rad
            };

            let d_outer = sdf_rr(rect_left, rect_top, rect_right, rect_bot, r_outer);
            let d_inner = sdf_rr(
                rect_left + stroke,
                rect_top + stroke,
                rect_right - stroke,
                rect_bot - stroke,
                r_inner.max(0.0),
            );

            let outer_cov = 0.5 - d_outer;
            let inner_cov = 0.5 - d_inner;
            let border_cov = outer_cov.min(1.0 - inner_cov.clamp(0.0, 1.0));
            if border_cov > 0.0 {
                blend(&mut data, x, y, border_cov, opacity);
            }
        }
    }

    // --- "C" glyph (arc spanning ~280°, opening on the right) ---
    let draw_c = |data: &mut Vec<u8>, center_x: f32, center_y: f32, op: f32| {
        let mid_r = 7.5_f32;
        let half_w = 2.8_f32;
        let arc_half_angle = 140.0_f32 * std::f32::consts::PI / 180.0;

        for y in 0..S {
            for x in 0..S {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let dx = px - center_x;
                let dy = py - center_y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < 0.01 {
                    continue;
                }
                let ring_dist = (dist - mid_r).abs();
                if ring_dist > half_w + 1.0 {
                    continue;
                }
                let angle = dy.atan2(dx);
                let in_arc = angle.abs() > (std::f32::consts::PI - arc_half_angle);
                if !in_arc {
                    continue;
                }
                let ring_cov = (half_w - ring_dist + 0.5).clamp(0.0, 1.0);
                let angle_from_end =
                    angle.abs() - (std::f32::consts::PI - arc_half_angle);
                let arc_cov = (angle_from_end * mid_r + 0.5).clamp(0.0, 1.0);
                let coverage = ring_cov.min(arc_cov);
                if coverage > 0.0 {
                    blend(data, x, y, coverage, op);
                }
            }
        }
    };

    draw_c(&mut data, 20.0, 32.0, opacity);
    draw_c(&mut data, 44.0, 32.0, opacity);

    // --- Diagonal strikethrough (bottom-left to top-right) ---
    if strikethrough {
        let line_width = 3.0_f32;
        for y in 0..S {
            for x in 0..S {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let dist = ((px + py) - SF).abs() / std::f32::consts::SQRT_2;
                if dist < line_width {
                    let cov = (line_width - dist).clamp(0.0, 1.0);
                    blend(&mut data, x, y, cov, opacity);
                }
            }
        }
    }

    data
}
