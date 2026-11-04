//! The app's icon, drawn in code so every tray host can show it with no icon theme: a webcam
//! with a round body, a lens and a stand. When the camera is on, a red eye glows in the lens.
//!
//! Dark shapes with a light outline, so it shows on dark and light panels alike.

/// Light outline, dark body, darker lens.
const OUTLINE: [f32; 3] = [0.92, 0.92, 0.92];
const BODY: [f32; 3] = [0.16, 0.16, 0.17];
const LENS: [f32; 3] = [0.07, 0.07, 0.08];
const EYE_OFF: [f32; 3] = [0.45, 0.45, 0.47];
const EYE_ON: [f32; 3] = [1.0, 0.13, 0.1];

/// Shapes are laid out on a 24×24 grid, scaled to the size asked for.
const GRID: f32 = 24.0;
const CENTRE: (f32, f32) = (12.0, 10.0);
const BODY_RADIUS: f32 = 8.5;
const LENS_RADIUS: f32 = 4.6;
const EYE_RADIUS: f32 = 2.1;
const LINE: f32 = 1.5;
const STAND: [(f32, f32); 4] = [(9.0, 17.0), (15.0, 17.0), (18.5, 22.5), (5.5, 22.5)];

/// The icon at `size`×`size` pixels, as ARGB32 bytes in network order (A, R, G, B), the format
/// StatusNotifierItem pixmaps use.
pub fn webcam(size: u32, on: bool) -> Vec<u8> {
    let scale = size as f32 / GRID;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            let [r, g, b, a] = pixel(p, scale, on);
            data.extend([a, r, g, b].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    data
}

/// The colour at grid point `p`, painted back to front.
fn pixel(p: (f32, f32), scale: f32, on: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    // How much of the pixel lies inside a shape `d` grid units from its edge (negative inside).
    let cover = |d: f32| (0.5 - d * scale).clamp(0.0, 1.0);
    let from_centre = distance(p, CENTRE);

    // Each outlined shape: the outline colour, then the fill colour inset by the line.
    let stand = polygon(p, &STAND);
    paint(&mut out, OUTLINE, cover(stand));
    paint(&mut out, BODY, cover(stand + LINE));
    let body = from_centre - BODY_RADIUS;
    paint(&mut out, OUTLINE, cover(body));
    paint(&mut out, BODY, cover(body + LINE));
    let lens = from_centre - LENS_RADIUS;
    paint(&mut out, OUTLINE, cover(lens));
    paint(&mut out, LENS, cover(lens + LINE));

    let eye = from_centre - EYE_RADIUS;
    if on {
        // A red glow fading out across the lens, then the eye and a small glint.
        let glow = (1.0 - eye / (LENS_RADIUS - LINE - EYE_RADIUS)).clamp(0.0, 1.0);
        paint(&mut out, EYE_ON, 0.6 * glow * glow * cover(lens + LINE));
        paint(&mut out, EYE_ON, cover(eye));
        let glint = distance(p, (CENTRE.0 - 0.7, CENTRE.1 - 0.7)) - 0.6;
        paint(&mut out, [1.0, 0.85, 0.8], cover(glint));
    } else {
        paint(&mut out, EYE_OFF, cover(eye));
    }
    out
}

/// Paints `colour` over `out` with `alpha`.
fn paint(out: &mut [f32; 4], colour: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let a = alpha + out[3] * (1.0 - alpha);
    for i in 0..3 {
        out[i] = (colour[i] * alpha + out[i] * out[3] * (1.0 - alpha)) / a;
    }
    out[3] = a;
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Distance from `p` to the edge of a convex or simple polygon; negative inside.
fn polygon(p: (f32, f32), corners: &[(f32, f32)]) -> f32 {
    let mut nearest = f32::MAX;
    let mut inside = false;
    for (i, &a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * ex + (p.1 - a.1) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        nearest = nearest.min(distance(p, (a.0 + ex * t, a.1 + ey * t)));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / ey * ex {
            inside = !inside;
        }
    }
    if inside { -nearest } else { nearest }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (A, R, G, B) of the pixel at grid point `p`.
    fn at(data: &[u8], size: u32, p: (f32, f32)) -> [u8; 4] {
        let (x, y) = (
            (p.0 * size as f32 / GRID) as u32,
            (p.1 * size as f32 / GRID) as u32,
        );
        let i = ((y * size + x) * 4) as usize;
        data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_eye_is_red_only_when_on() {
        for size in [22, 48] {
            let on = webcam(size, true);
            let off = webcam(size, false);
            assert_eq!(on.len(), (size * size * 4) as usize);
            let [a, r, g, _] = at(&on, size, (CENTRE.0 + 1.0, CENTRE.1 + 1.0));
            assert!(a == 255 && r > 200 && g < 80, "red eye at {size}px");
            let [_, r, g, _] = at(&off, size, (CENTRE.0 + 1.0, CENTRE.1 + 1.0));
            assert!(r.abs_diff(g) < 10, "grey eye at {size}px");
            assert_eq!(at(&off, size, (0.5, 0.5))[0], 0, "corners are clear");
            assert_eq!(at(&off, size, (12.0, 21.0))[0], 255, "the stand is there");
        }
    }
}
