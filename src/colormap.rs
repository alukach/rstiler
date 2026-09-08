//! Built-in colormaps, keyed by the names titiler uses.
//!
//! Each map is a list of `(position, r, g, b)` control points that are
//! linearly interpolated at lookup. The perceptual maps come from
//! matplotlib (CC0); resampling their 256 entries down to 43 control points
//! reproduces them to within a rounding step, so the table stays under a
//! kilobyte. `terrain` keeps matplotlib's own segment stops, exactly.

type Stops = &'static [(u8, u8, u8, u8)];

// Generated data, deliberately dense — one map per line reads as a table.
#[rustfmt::skip]
pub(crate) const BUILTIN: &[(&str, Stops)] = &[
    // viridis: 43 stops, max channel error 1
    ("viridis", &[(0,68,1,84), (6,70,10,93), (12,71,19,101), (18,72,27,109), (24,72,35,116), (30,71,42,122), (36,70,50,126), (42,68,57,131), (49,66,65,134), (55,63,72,137), (61,60,79,138), (67,57,85,140), (73,54,92,141), (79,51,98,141), (85,49,104,142), (91,46,110,142), (97,44,115,142), (103,41,121,142), (109,39,127,142), (115,37,132,142), (121,35,138,141), (128,33,145,140), (134,31,150,139), (140,30,156,137), (146,31,161,135), (152,34,167,133), (158,38,173,129), (164,45,178,125), (170,53,183,121), (176,63,188,115), (182,74,193,109), (188,86,198,103), (194,99,203,95), (200,112,207,87), (206,127,211,78), (212,142,214,69), (219,160,218,57), (225,176,221,47), (231,192,223,37), (237,208,225,28), (243,223,227,24), (249,239,229,28), (255,253,231,37)]),
    // magma: 43 stops, max channel error 1
    ("magma", &[(0,0,0,4), (6,2,2,13), (12,6,5,26), (18,12,9,38), (24,19,13,52), (30,26,16,66), (36,34,17,80), (42,44,17,95), (49,56,16,108), (55,66,15,117), (61,76,17,122), (67,86,20,125), (73,95,24,127), (79,104,28,129), (85,114,31,129), (91,123,35,130), (97,132,38,129), (103,142,42,129), (109,152,45,128), (115,161,48,126), (121,171,51,124), (128,183,55,121), (134,192,58,118), (140,202,62,114), (146,211,67,110), (152,220,72,105), (158,228,79,100), (164,235,87,96), (170,241,96,93), (176,245,107,92), (182,248,118,92), (188,250,129,95), (194,252,140,99), (200,253,152,105), (206,254,163,111), (212,254,174,119), (219,254,187,129), (225,254,198,138), (231,254,209,148), (237,253,220,158), (243,253,231,169), (249,252,242,180), (255,252,253,191)]),
    // plasma: 43 stops, max channel error 2
    ("plasma", &[(0,13,8,135), (6,29,6,142), (12,42,5,147), (18,53,4,152), (24,63,4,156), (30,73,3,160), (36,83,2,163), (42,92,1,166), (49,103,0,168), (55,113,0,168), (61,122,2,168), (67,131,5,167), (73,139,10,165), (79,148,16,162), (85,156,23,158), (91,163,30,154), (97,171,36,148), (103,178,43,143), (109,184,50,137), (115,191,57,132), (121,197,64,126), (128,204,71,120), (134,209,78,114), (140,214,85,109), (146,219,92,104), (152,224,99,99), (158,229,106,93), (164,233,113,88), (170,237,121,83), (176,240,128,78), (182,244,136,73), (188,247,144,68), (194,249,152,62), (200,251,161,57), (206,252,169,52), (212,253,178,47), (219,254,189,42), (225,253,198,39), (231,252,208,37), (237,250,218,36), (243,247,228,37), (249,243,238,39), (255,240,249,33)]),
    // inferno: 43 stops, max channel error 1
    ("inferno", &[(0,0,0,4), (6,2,2,14), (12,7,5,27), (18,13,8,41), (24,21,11,55), (30,30,12,69), (36,40,11,83), (42,50,10,94), (49,62,9,102), (55,73,11,106), (61,82,14,109), (67,92,18,110), (73,101,21,110), (79,111,25,110), (85,120,28,109), (91,130,32,108), (97,140,35,105), (103,149,38,103), (109,159,42,99), (115,168,46,95), (121,177,50,90), (128,188,55,84), (134,196,60,78), (140,204,66,72), (146,212,72,66), (152,219,80,59), (158,226,87,52), (164,232,96,45), (170,237,105,37), (176,241,115,29), (182,245,125,21), (188,248,135,14), (194,250,146,7), (200,251,157,7), (206,252,168,13), (212,252,180,24), (219,250,194,40), (225,248,205,55), (231,245,217,73), (237,243,229,93), (243,241,239,117), (249,244,248,142), (255,252,255,164)]),
    // cividis: 43 stops, max channel error 4
    ("cividis", &[(0,0,34,78), (6,0,39,88), (12,0,43,98), (18,0,47,109), (24,5,51,113), (30,22,55,112), (36,33,59,110), (42,42,63,109), (49,51,68,109), (55,58,72,108), (61,64,76,108), (67,70,81,108), (73,76,85,108), (79,82,89,109), (85,87,93,109), (91,93,97,110), (97,98,101,111), (103,103,106,113), (109,108,110,114), (115,114,114,116), (121,119,119,118), (128,125,124,120), (134,130,128,121), (140,136,133,120), (146,142,137,120), (152,147,142,120), (158,153,146,119), (164,159,151,117), (170,165,156,116), (176,171,160,114), (182,177,165,112), (188,184,170,110), (194,190,175,107), (200,196,180,104), (206,203,185,101), (212,209,191,97), (219,217,197,92), (225,223,202,87), (231,230,208,81), (237,237,213,74), (243,243,219,66), (249,251,225,56), (255,254,232,56)]),
    // terrain: 6 stops, max channel error 0
    ("terrain", &[(0,51,51,153), (38,0,153,255), (64,0,204,102), (128,255,255,153), (191,128,92,84), (255,255,255,255)]),
    // greys: 2 stops, max channel error 0
    ("greys", &[(0,0,0,0), (255,255,255,255)]),
];

use std::collections::HashMap;

/// How a single-band value becomes a colour.
pub(crate) enum Colormap {
    /// `colormap_name=viridis` — interpolated across the rescaled 0..255 range.
    Continuous(Stops),
    /// `colormap={"11":[70,107,159,255],...}` — looked up on the *raw* value,
    /// before any rescale. This is what categorical rasters need.
    Discrete(HashMap<i64, [u8; 4]>),
}

impl Colormap {
    /// `rescaled` is the 0..255 value for a continuous map; `raw` is the
    /// untouched sample, which is what a discrete map keys on.
    pub(crate) fn lookup(&self, rescaled: u8, raw: f64) -> Option<[u8; 4]> {
        match self {
            Self::Discrete(table) => table.get(&(raw.round() as i64)).copied(),
            Self::Continuous(stops) => {
                let window = stops
                    .windows(2)
                    .find(|w| rescaled >= w[0].0 && rescaled <= w[1].0)?;
                let (p0, r0, g0, b0) = window[0];
                let (p1, r1, g1, b1) = window[1];
                let span = (p1 - p0) as f32;
                let f = if span == 0.0 {
                    0.0
                } else {
                    (rescaled - p0) as f32 / span
                };
                let mix = |a: u8, b: u8| (a as f32 + f * (b as f32 - a as f32)).round() as u8;
                Some([mix(r0, r1), mix(g0, g1), mix(b0, b1), 255])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(name: &str) -> Colormap {
        let (_, stops) = BUILTIN.iter().find(|(n, _)| *n == name).unwrap();
        Colormap::Continuous(stops)
    }

    #[test]
    fn every_builtin_spans_the_full_range() {
        for (name, stops) in BUILTIN {
            assert_eq!(stops[0].0, 0, "{name} must start at 0");
            assert_eq!(stops[stops.len() - 1].0, 255, "{name} must end at 255");
            assert!(
                stops.windows(2).all(|w| w[0].0 < w[1].0),
                "{name} must ascend"
            );
        }
    }

    #[test]
    fn continuous_lookup_hits_the_endpoints() {
        // viridis runs dark purple to yellow-green.
        let [r, g, b, a] = map("viridis").lookup(0, 0.0).unwrap();
        assert_eq!((r, g, b), (68, 1, 84));
        assert_eq!(a, 255);
        let [r, g, b, _] = map("viridis").lookup(255, 0.0).unwrap();
        assert!(r > 240 && g > 220 && b < 60, "got {r},{g},{b}");
    }

    #[test]
    fn every_input_resolves() {
        for (name, stops) in BUILTIN {
            let cm = Colormap::Continuous(stops);
            for v in 0..=255u8 {
                assert!(cm.lookup(v, 0.0).is_some(), "{name} missed {v}");
            }
        }
    }

    #[test]
    fn discrete_lookup_is_exact_and_can_miss() {
        let mut t = HashMap::new();
        t.insert(11, [70, 107, 159, 255]);
        let cm = Colormap::Discrete(t);
        assert_eq!(cm.lookup(0, 11.0), Some([70, 107, 159, 255]));
        assert_eq!(
            cm.lookup(0, 11.4),
            Some([70, 107, 159, 255]),
            "rounds to nearest"
        );
        assert_eq!(cm.lookup(0, 12.0), None, "unmapped values stay transparent");
    }
}
