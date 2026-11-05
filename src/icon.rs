//! The app logo: a little sail on a rounded tile, drawn with egui shapes and
//! rasterised for the window icon.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};

const TILE_TOP: Color32 = Color32::from_rgb(0x6d, 0x8d, 0xff);
const TILE_BOTTOM: Color32 = Color32::from_rgb(0x8b, 0x5c, 0xf6);

pub fn paint_logo(p: &Painter, r: Rect) {
    let rounding = r.width() * 0.26;
    p.rect_filled(r, rounding, TILE_TOP);
    // subtle second tone on the lower half
    let lower = Rect::from_min_max(egui::pos2(r.min.x, r.center().y), r.max);
    p.rect_filled(
        lower,
        egui::CornerRadius { nw: 0, ne: 0, sw: rounding as u8, se: rounding as u8 },
        TILE_BOTTOM.gamma_multiply(0.55),
    );
    let s = r.width();
    let at = |x: f32, y: f32| Pos2::new(r.min.x + x * s, r.min.y + y * s);
    // sails
    p.add(Shape::convex_polygon(
        vec![at(0.50, 0.18), at(0.50, 0.64), at(0.24, 0.64)],
        Color32::WHITE,
        Stroke::NONE,
    ));
    p.add(Shape::convex_polygon(
        vec![at(0.55, 0.26), at(0.76, 0.64), at(0.55, 0.64)],
        Color32::from_white_alpha(200),
        Stroke::NONE,
    ));
    // hull
    p.add(Shape::convex_polygon(
        vec![at(0.18, 0.70), at(0.82, 0.70), at(0.70, 0.82), at(0.30, 0.82)],
        Color32::WHITE,
        Stroke::NONE,
    ));
}

/// Window icon (simple software rasteriser of the same design).
pub fn icon_data() -> egui::IconData {
    let n = 128usize;
    let mut rgba = vec![0u8; n * n * 4];
    let inside_round = |x: f32, y: f32| {
        let r = 0.26;
        let cx = x.clamp(r, 1.0 - r);
        let cy = y.clamp(r, 1.0 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    let tri = |p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let s = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
        let d1 = s(p, a, b);
        let d2 = s(p, b, c);
        let d3 = s(p, c, a);
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    for yi in 0..n {
        for xi in 0..n {
            // 4x supersampling
            let mut acc = [0f32; 4];
            for sy in 0..2 {
                for sx in 0..2 {
                    let x = (xi as f32 + 0.25 + sx as f32 * 0.5) / n as f32;
                    let y = (yi as f32 + 0.25 + sy as f32 * 0.5) / n as f32;
                    if !inside_round(x, y) {
                        continue;
                    }
                    let t = y;
                    let mut c = [
                        TILE_TOP.r() as f32 * (1.0 - t * 0.5) + TILE_BOTTOM.r() as f32 * t * 0.5,
                        TILE_TOP.g() as f32 * (1.0 - t * 0.5) + TILE_BOTTOM.g() as f32 * t * 0.5,
                        TILE_TOP.b() as f32 * (1.0 - t * 0.5) + TILE_BOTTOM.b() as f32 * t * 0.5,
                    ];
                    let p = (x, y);
                    let white = tri(p, (0.50, 0.18), (0.50, 0.64), (0.24, 0.64))
                        || tri(p, (0.18, 0.70), (0.82, 0.70), (0.70, 0.82))
                        || tri(p, (0.18, 0.70), (0.70, 0.82), (0.30, 0.82));
                    let light = tri(p, (0.55, 0.26), (0.76, 0.64), (0.55, 0.64));
                    if white {
                        c = [255.0; 3];
                    } else if light {
                        c = [c[0] * 0.2 + 204.0, c[1] * 0.2 + 204.0, c[2] * 0.2 + 204.0];
                    }
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                    acc[3] += 255.0;
                }
            }
            let i = (yi * n + xi) * 4;
            let cov = acc[3] / 255.0;
            if cov > 0.0 {
                rgba[i] = (acc[0] / cov) as u8;
                rgba[i + 1] = (acc[1] / cov) as u8;
                rgba[i + 2] = (acc[2] / cov) as u8;
            }
            rgba[i + 3] = (acc[3] / 4.0) as u8;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
