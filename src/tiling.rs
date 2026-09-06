//! Web Mercator <-> COG pixel math. Pure functions, no deps, so it can be
//! test-compiled on its own: `rustc --test src/tiling.rs -o /tmp/t && /tmp/t`

/// Half the circumference of the earth at the equator, in EPSG:3857 metres.
pub const R: f64 = 20_037_508.342_789_244;

/// Bounds of an XYZ tile in EPSG:3857 metres: (xmin, ymin, xmax, ymax).
pub fn tile_bounds(z: u32, x: u32, y: u32) -> (f64, f64, f64, f64) {
    let span = 2.0 * R / (1u64 << z) as f64;
    let xmin = -R + x as f64 * span;
    let ymax = R - y as f64 * span;
    (xmin, ymax - span, xmin + span, ymax)
}

/// Affine transform of a north-up GeoTIFF, derived from ModelPixelScale +
/// ModelTiepoint. `res` is metres per pixel at full resolution.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub origin_x: f64,
    pub origin_y: f64,
    pub res_x: f64,
    pub res_y: f64,
}

impl Transform {
    /// `scale` is the overview downsample factor (1.0 for the full-res IFD).
    pub fn world_to_pixel(&self, wx: f64, wy: f64, scale: f64) -> (f64, f64) {
        (
            (wx - self.origin_x) / (self.res_x * scale),
            (self.origin_y - wy) / (self.res_y * scale),
        )
    }
}

/// Index of the coarsest overview whose resolution is still finer than what the
/// output tile needs. `widths[0]` must be the full-resolution IFD.
pub fn pick_overview(widths: &[u32], full_res: f64, target_res: f64) -> usize {
    let mut best = 0;
    for (i, w) in widths.iter().enumerate() {
        let scale = widths[0] as f64 / *w as f64;
        if full_res * scale <= target_res && scale >= widths[0] as f64 / widths[best] as f64 {
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_0_0_0_is_the_whole_world() {
        let (xmin, ymin, xmax, ymax) = tile_bounds(0, 0, 0);
        assert!((xmin + R).abs() < 1e-6 && (xmax - R).abs() < 1e-6);
        assert!((ymin + R).abs() < 1e-6 && (ymax - R).abs() < 1e-6);
    }

    #[test]
    fn y_increases_downward() {
        let top = tile_bounds(1, 0, 0);
        let bottom = tile_bounds(1, 0, 1);
        assert!(top.1 > bottom.1, "row 0 must sit above row 1");
        assert!((top.1 - bottom.3).abs() < 1e-6, "rows must abut");
    }

    #[test]
    fn pixel_lookup_round_trips() {
        let t = Transform { origin_x: -100.0, origin_y: 200.0, res_x: 10.0, res_y: 10.0 };
        assert_eq!(t.world_to_pixel(-100.0, 200.0, 1.0), (0.0, 0.0));
        assert_eq!(t.world_to_pixel(-50.0, 150.0, 1.0), (5.0, 5.0));
        // an overview at half resolution puts the same point at half the index
        assert_eq!(t.world_to_pixel(-50.0, 150.0, 2.0), (2.5, 2.5));
    }

    #[test]
    fn overview_choice_tracks_zoom() {
        let widths = [1000, 500, 250]; // full res 10 m/px
        assert_eq!(pick_overview(&widths, 10.0, 5.0), 0, "zoomed in: full res");
        assert_eq!(pick_overview(&widths, 10.0, 20.0), 1, "2x out: first overview");
        assert_eq!(pick_overview(&widths, 10.0, 9999.0), 2, "zoomed way out: coarsest");
    }
}
