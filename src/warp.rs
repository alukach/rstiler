//! Mapping output pixels to source-CRS coordinates, cheaply.
//!
//! Transforming every pixel is exact and costs ~65k projections per tile.
//! Measured on one fixture rendered twice — once where the source is already
//! Web Mercator and no transform runs, once where it is UTM — that is 3 ms of
//! render versus 21 ms. On Cloudflare Workers the free CPU budget for a whole
//! request is 10 ms, so the projection alone can blow it.
//!
//! GDAL's answer is to approximate the transform over a coarse grid and
//! interpolate between the nodes, subdividing where the approximation drifts.
//! This does the same, with one difference: rather than subdividing, it checks
//! the approximation against the exact transform at every cell centre and
//! falls back to exact everywhere if any cell is out of tolerance. Simpler,
//! and the check costs one extra projection per cell.

use crate::geo::Reproject;

/// Output pixels between grid nodes. 16 gives a 17x17 grid over a 256px tile:
/// 289 nodes plus 256 verification points, against 65,536 exact transforms.
const STEP: usize = 16;

/// How far the approximation may drift, in source pixels. `gdalwarp` uses the
/// same figure as its default error threshold.
const TOLERANCE_PX: f64 = 0.125;

pub(crate) struct Warp<'a> {
    reproject: &'a Reproject,
    /// Output pixel (0..size) to Web Mercator metres.
    xmin: f64,
    ymax: f64,
    mx: f64,
    my: f64,
    size: usize,
    /// (size/STEP + 1)^2 transformed nodes, row-major. `None` where the point
    /// falls outside the target projection's domain.
    nodes: Vec<Option<(f64, f64)>>,
    across: usize,
    /// False when the grid could not be trusted and every pixel is transformed.
    approximate: bool,
}

impl<'a> Warp<'a> {
    /// `bounds` is the tile in Web Mercator metres; `source_px` is the size of
    /// one source pixel in the source CRS's own units, which is what the
    /// tolerance is measured against.
    pub(crate) fn new(
        reproject: &'a Reproject,
        bounds: (f64, f64, f64, f64),
        size: usize,
        source_px: f64,
    ) -> Self {
        let (xmin, ymin, xmax, ymax) = bounds;
        let across = size / STEP + 1;
        let mut w = Self {
            reproject,
            xmin,
            ymax,
            mx: (xmax - xmin) / size as f64,
            my: (ymax - ymin) / size as f64,
            size,
            nodes: Vec::with_capacity(across * across),
            across,
            approximate: false,
        };

        for j in 0..across {
            for i in 0..across {
                let (ox, oy) = ((i * STEP) as f64, (j * STEP) as f64);
                w.nodes.push(w.exact(ox, oy));
            }
        }

        // Trust the grid only where it demonstrably agrees with the real
        // transform. One probe per cell, at its centre, where bilinear
        // interpolation sits furthest from the nodes it was built from.
        let tol = TOLERANCE_PX * source_px;
        w.approximate = true;
        'check: for j in 0..across - 1 {
            for i in 0..across - 1 {
                let (ox, oy) = (
                    (i * STEP) as f64 + STEP as f64 / 2.0,
                    (j * STEP) as f64 + STEP as f64 / 2.0,
                );
                match (w.interpolate(ox, oy), w.exact(ox, oy)) {
                    (Some((ax, ay)), Some((ex, ey))) => {
                        if (ax - ex).abs() > tol || (ay - ey).abs() > tol {
                            w.approximate = false;
                            break 'check;
                        }
                    }
                    // A cell straddling the edge of the projection's domain is
                    // exactly where interpolation misleads. Do it properly.
                    (None, Some(_)) | (Some(_), None) => {
                        w.approximate = false;
                        break 'check;
                    }
                    (None, None) => {}
                }
            }
        }
        w
    }

    /// Source-CRS coordinate for the centre of output pixel (ox, oy).
    pub(crate) fn at(&self, ox: f64, oy: f64) -> Option<(f64, f64)> {
        if self.approximate {
            if let Some(p) = self.interpolate(ox, oy) {
                return Some(p);
            }
        }
        self.exact(ox, oy)
    }

    /// True when the grid was accepted. For reporting, not control flow.
    pub(crate) fn is_approximate(&self) -> bool {
        self.approximate
    }

    fn exact(&self, ox: f64, oy: f64) -> Option<(f64, f64)> {
        self.reproject.apply(
            self.xmin + (ox + 0.5) * self.mx,
            self.ymax - (oy + 0.5) * self.my,
        )
    }

    /// Bilinear interpolation between the four surrounding grid nodes.
    /// `None` if any of them is outside the projection's domain.
    fn interpolate(&self, ox: f64, oy: f64) -> Option<(f64, f64)> {
        // Clamp the *cell*, not the coordinate: clamping the coordinate first
        // would flatten the fraction in the last row and column, and the tile
        // edge would interpolate against a single node.
        let last = self.across - 2;
        let (gx, gy) = (ox / STEP as f64, oy / STEP as f64);
        let (i, j) = (
            (gx.floor().max(0.0) as usize).min(last),
            (gy.floor().max(0.0) as usize).min(last),
        );
        let (fx, fy) = (gx - i as f64, gy - j as f64);

        let node = |i: usize, j: usize| self.nodes[j * self.across + i];
        let (a, b) = (node(i, j)?, node(i + 1, j)?);
        let (c, d) = (node(i, j + 1)?, node(i + 1, j + 1)?);

        let mix = |p: f64, q: f64, f: f64| p + f * (q - p);
        Some((
            mix(mix(a.0, b.0, fx), mix(c.0, d.0, fx), fy),
            mix(mix(a.1, b.1, fx), mix(c.1, d.1, fx), fy),
        ))
    }
}

impl std::fmt::Debug for Warp<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Warp({}px, {})",
            self.size,
            if self.approximate { "grid" } else { "exact" }
        )
    }
}
