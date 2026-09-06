//! The `/cog/tiles/{z}/{x}/{y}.png` handler: the actual resampling pipeline.

use std::collections::HashMap;

use async_tiff::tags::PlanarConfiguration;
use async_tiff::TypedArray;
use worker::*;

use crate::cog::{decoders, image_ifds, nodata_of, open};
use crate::geo::{transform_of, source_crs, Reproject};
use crate::render::{band_indices, colormap, png_response, rescale, resampling, sample};
use crate::tiling::{pick_overview, tile_bounds};
use crate::{MAX_SOURCE_TILES, TILE};

pub(crate) async fn tile(
    src: &str,
    z: &str,
    x: &str,
    y: &str,
    q: &HashMap<String, String>,
) -> Result<Response> {
    let parse = |s: &str, what: &str| -> Result<u32> {
        s.trim_end_matches(".png")
            .parse()
            .map_err(|_| Error::RustError(format!("bad {what}")))
    };
    let (z, x, y) = (parse(z, "z")?, parse(x, "x")?, parse(y, "y")?);

    let t0 = Date::now().as_millis();
    let (reader, tiff) = open(src).await?;
    let t_meta = Date::now().as_millis() - t0;
    let ifds = image_ifds(&tiff);
    let full = ifds[0];
    let t = transform_of(full)?;
    let reproject = Reproject::new(&source_crs(full)?)?;

    if full.planar_configuration() != PlanarConfiguration::Chunky {
        // ponytail: planar TIFFs are rare in the wild; add band-plane
        // reassembly here if one shows up.
        return Err(Error::RustError("planar TIFFs not supported".into()));
    }

    let (xmin, ymin, xmax, ymax) = tile_bounds(z, x, y);
    let blank = || png_response(&vec![0u8; TILE * TILE * 4]);

    // Source-CRS coordinate for an output pixel centre.
    let at = |ox: f64, oy: f64| {
        reproject.apply(
            xmin + (ox + 0.5) * (xmax - xmin) / TILE as f64,
            ymax - (oy + 0.5) * (ymax - ymin) / TILE as f64,
        )
    };

    // Probe a coarse grid to size the read window and the needed resolution.
    // Corners alone would miss the bulge in a reprojected tile's edges.
    const PROBE: usize = 8;
    let mut probes = Vec::with_capacity((PROBE + 1) * (PROBE + 1));
    for i in 0..=PROBE {
        for j in 0..=PROBE {
            let (ox, oy) = (
                i as f64 * TILE as f64 / PROBE as f64,
                j as f64 * TILE as f64 / PROBE as f64,
            );
            if let Some(p) = at(ox, oy) {
                probes.push(p);
            }
        }
    }
    if probes.is_empty() {
        return blank(); // tile lies entirely outside the projection's domain
    }
    let (wx0, wx1) = (
        probes.iter().map(|p| p.0).fold(f64::MAX, f64::min),
        probes.iter().map(|p| p.0).fold(f64::MIN, f64::max),
    );
    let (wy0, wy1) = (
        probes.iter().map(|p| p.1).fold(f64::MAX, f64::min),
        probes.iter().map(|p| p.1).fold(f64::MIN, f64::max),
    );

    // Resolution the output needs, expressed in the source CRS's own units.
    let target_res = ((wx1 - wx0) / TILE as f64).max((wy1 - wy0) / TILE as f64);
    let widths: Vec<u32> = ifds.iter().map(|i| i.image_width()).collect();
    let level = pick_overview(&widths, t.res_x, target_res);
    let ifd = ifds[level];
    let scale = widths[0] as f64 / widths[level] as f64;

    let colormap = colormap(q)?;
    let bands = band_indices(q, ifd.samples_per_pixel() as usize)?;
    if colormap.is_some() && bands.len() != 1 {
        return Err(Error::RustError(
            "a colormap needs exactly one band; pass bidx=<n>".into(),
        ));
    }
    let (lo, hi) = rescale(q, bands.len())?;
    let how = resampling(q)?;
    // titiler lets a request override the dataset's own nodata.
    let nodata = match q.get("nodata").filter(|s| !s.is_empty()) {
        Some(s) => Some(
            s.trim()
                .parse::<f64>()
                .map_err(|_| Error::RustError(format!("nodata {s:?} is not a number")))?,
        ),
        None => nodata_of(ifd),
    };

    let (iw, ih) = (ifd.image_width() as f64, ifd.image_height() as f64);
    let (px0, py0) = t.world_to_pixel(wx0, wy1, scale);
    let (px1, py1) = t.world_to_pixel(wx1, wy0, scale);
    if px1 < 0.0 || py1 < 0.0 || px0 > iw || py0 > ih {
        return blank();
    }

    let tw = ifd
        .tile_width()
        .ok_or_else(|| Error::RustError("not a tiled TIFF (stripped COGs unsupported)".into()))?
        as usize;
    let th = ifd.tile_height().unwrap() as usize;
    let (ntx, nty) = ifd.tile_count().unwrap();

    let clamp = |v: f64, n: usize| (v.max(0.0) as usize).min(n.saturating_sub(1));
    let (tx0, tx1) = (clamp(px0 / tw as f64, ntx), clamp(px1 / tw as f64, ntx));
    let (ty0, ty1) = (clamp(py0 / th as f64, nty), clamp(py1 / th as f64, nty));

    let coords: Vec<(usize, usize)> = (ty0..=ty1)
        .flat_map(|ty| (tx0..=tx1).map(move |tx| (tx, ty)))
        .collect();
    if coords.len() > MAX_SOURCE_TILES {
        return Err(Error::RustError(format!(
            "tile would need {} source tiles (max {MAX_SOURCE_TILES}); the COG is probably missing overviews",
            coords.len()
        )));
    }

    let t1 = Date::now().as_millis();
    let fetched = ifd
        .fetch_tiles(&coords, &reader)
        .await
        .map_err(|e| Error::RustError(format!("tile fetch failed: {e}")))?;
    let t_fetch = Date::now().as_millis() - t1;
    let t2 = Date::now().as_millis();

    let mut chunks: HashMap<(usize, usize), (TypedArray, usize)> = HashMap::new();
    for tile in fetched {
        let (cx, cy) = (tile.x(), tile.y());
        let arr = tile
            .decode(&decoders())
            .map_err(|e| Error::RustError(format!("decode failed: {e}")))?;
        let nb = arr.shape()[2];
        chunks.insert((cx, cy), (arr.into_inner().0, nb));
    }

    // Nearest-neighbour resample into the output tile.
    // ponytail: exact reprojection per pixel, and nearest only. GDAL
    // interpolates the transform over a coarse grid; do that if CPU bites.
    // One source sample, or None when it is outside the image, in an unfetched
    // chunk, or nodata.
    let at_px = |sx: usize, sy: usize, band: usize| -> Option<f64> {
        let (data, nb) = chunks.get(&(sx / tw, sy / th))?;
        let v = sample(data, ((sy % th) * tw + (sx % tw)) * nb + band);
        (v.is_finite() && nodata != Some(v)).then_some(v)
    };

    let mut rgba = vec![0u8; TILE * TILE * 4];
    for oy in 0..TILE {
        for ox in 0..TILE {
            let Some((wx, wy)) = at(ox as f64, oy as f64) else {
                continue;
            };
            let (fx, fy) = t.world_to_pixel(wx, wy, scale);
            if fx < 0.0 || fy < 0.0 || fx >= iw || fy >= ih {
                continue;
            }

            let o = (oy * TILE + ox) * 4;
            let mut raw = [0.0f64; 3];
            let mut eight = [0u8; 3];
            let mut valid = true;
            for (c, b) in bands.iter().enumerate() {
                let Some(v) = how.sample(fx, fy, iw, ih, |x, y| at_px(x, y, *b)) else {
                    valid = false;
                    break;
                };
                let (l, h) = (lo[c], hi[c]);
                raw[c] = v;
                eight[c] = (((v - l) / (h - l)).clamp(0.0, 1.0) * 255.0) as u8;
            }
            if !valid {
                continue;
            }

            match &colormap {
                Some(cm) => match cm.lookup(eight[0], raw[0]) {
                    Some(px) => rgba[o..o + 4].copy_from_slice(&px),
                    None => continue, // unmapped category stays transparent
                },
                None => {
                    rgba[o..o + 3].copy_from_slice(&eight);
                    if bands.len() == 1 {
                        rgba[o + 1] = eight[0];
                        rgba[o + 2] = eight[0];
                    }
                    rgba[o + 3] = 255;
                }
            }
        }
    }

    // Where the time went, per tile: metadata walk, source reads, resample.
    let timing = format!(
        "meta;dur={t_meta}, fetch;dur={t_fetch};desc=\"{} tiles\", render;dur={}",
        coords.len(),
        Date::now().as_millis() - t2
    );
    let mut resp = png_response(&rgba)?;
    resp.headers_mut().set("Server-Timing", &timing)?;
    Ok(resp)
}
