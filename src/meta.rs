//! The metadata endpoints: `/cog/info` and `/cog/tilejson.json`.

use async_tiff::ImageFileDirectory;
use worker::*;

use crate::cog::{decoders, image_ifds, nodata_of, open, HttpReader};
use crate::geo::{source_crs, transform_of, wgs84_bounds, Crs, Reproject};
use crate::render::sample;
use crate::{tiling, STATS_TILES, TILE};

pub(crate) async fn info(src: &str) -> Result<Response> {
    let (reader, tiff) = open(src).await?;
    let ifds = image_ifds(&tiff);
    let ifd = ifds[0];
    let t = transform_of(ifd)?;
    let crs = source_crs(ifd)?;
    let bands = ifd.samples_per_pixel() as usize;

    Response::from_json(&serde_json::json!({
        "width": ifd.image_width(),
        "height": ifd.image_height(),
        "band_count": bands,
        "bits_per_sample": ifd.bits_per_sample(),
        "sample_format": ifd.sample_format().first().map(|f| format!("{f:?}")),
        "compression": format!("{:?}", ifd.compression()),
        "nodata": ifd.gdal_nodata(),
        "crs": crs.label(),
        "overviews": ifds.len() - 1,
        "resolution": [t.res_x, t.res_y],
        "bounds": wgs84_bounds(&t, &crs, ifd.image_width(), ifd.image_height())?,
    }))
    .map(|r| timed(r, &reader))
}

/// Attaches the range-request accounting that explains a response's latency.
fn timed(mut resp: Response, reader: &HttpReader) -> Response {
    let (reqs, hits) = reader.traffic();
    let _ = resp.headers_mut().set(
        "Server-Timing",
        &format!("origin;desc=\"{} range reads, {hits} cached\"", reqs - hits),
    );
    resp
}

/// `/cog/statistics` — per-band summary, read off the coarsest overview.
///
/// Split out of `/cog/info` deliberately: a COG with no overview pyramid makes
/// this a full-resolution read, and origins charge ~2 s per range request. Keep
/// `/cog/info` to metadata a client always needs.
pub(crate) async fn statistics(src: &str) -> Result<Response> {
    let (reader, tiff) = open(src).await?;
    let ifds = image_ifds(&tiff);
    let bands = ifds[0].samples_per_pixel() as usize;
    let stats = percentiles(ifds[ifds.len() - 1], &reader, nodata_of(ifds[0]), bands).await?;

    let body: serde_json::Map<String, serde_json::Value> = stats
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("b{}", i + 1), serde_json::to_value(s).unwrap()))
        .collect();
    Response::from_json(&body).map(|r| timed(r, &reader))
}

/// Per-band summary, computed from a sample of one overview level.
#[derive(serde::Serialize)]
pub(crate) struct BandStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub percentile_2: f64,
    pub percentile_98: f64,
    /// Samples actually examined — this is a sample, not a full pass.
    pub count: usize,
}

pub(crate) async fn percentiles(
    ifd: &ImageFileDirectory,
    reader: &HttpReader,
    nodata: Option<f64>,
    bands: usize,
) -> Result<Vec<BandStats>> {
    let Some((ntx, nty)) = ifd.tile_count() else {
        return Ok(vec![]);
    };
    let all: Vec<(usize, usize)> = (0..nty).flat_map(|y| (0..ntx).map(move |x| (x, y))).collect();
    // Spread the sample across the image rather than taking a corner of it.
    let step = all.len().div_ceil(STATS_TILES).max(1);
    let coords: Vec<(usize, usize)> = all.into_iter().step_by(step).take(STATS_TILES).collect();

    let tiles = ifd
        .fetch_tiles(&coords, reader)
        .await
        .map_err(|e| Error::RustError(format!("stats fetch failed: {e}")))?;

    let mut per_band: Vec<Vec<f64>> = vec![Vec::new(); bands];
    for t in tiles {
        let arr = t
            .decode(&decoders())
            .map_err(|e| Error::RustError(format!("stats decode failed: {e}")))?;
        let nb = arr.shape()[2];
        let (data, _, _) = arr.into_inner();
        // One value in every 16 pixels is plenty for a percentile, and keeps
        // the sort off the hot path on a full-resolution tile.
        for i in (0..data.len()).step_by(nb * 16) {
            for b in 0..bands.min(nb) {
                let v = sample(&data, i + b);
                if v.is_finite() && nodata != Some(v) {
                    per_band[b].push(v);
                }
            }
        }
    }

    Ok(per_band
        .into_iter()
        .map(|mut v| {
            if v.is_empty() {
                return BandStats {
                    min: 0.0, max: 255.0, mean: 0.0,
                    percentile_2: 0.0, percentile_98: 255.0, count: 0,
                };
            }
            v.sort_by(|a, b| a.total_cmp(b));
            let at = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
            let (lo, hi) = (at(0.02), at(0.98));
            BandStats {
                min: v[0],
                max: v[v.len() - 1],
                mean: v.iter().sum::<f64>() / v.len() as f64,
                percentile_2: lo,
                // Keep the range non-empty so it is always usable as a rescale.
                percentile_98: if hi > lo { hi } else { lo + 1.0 },
                count: v.len(),
            }
        })
        .collect())
}

pub(crate) async fn tilejson(src: &str, req_url: &Url) -> Result<Response> {
    let (reader, tiff) = open(src).await?;
    let ifds = image_ifds(&tiff);
    let ifd = ifds[0];
    let t = transform_of(ifd)?;
    let crs = source_crs(ifd)?;
    let b = wgs84_bounds(&t, &crs, ifd.image_width(), ifd.image_height())?;

    // Ground resolution at the image centre, in Web Mercator metres per pixel:
    // mercator stretches by 1/cos(lat), so a metre-based CRS needs scaling.
    let merc_res = if crs == Crs::WGS84 {
        t.res_x * tiling::R / 180.0 // degrees of longitude -> mercator metres
    } else {
        let lat = ((b[1] + b[3]) / 2.0).to_radians();
        t.res_x / lat.cos() // mercator stretches by 1/cos(lat)
    };
    let maxzoom =
        ((2.0 * tiling::R / (TILE as f64 * merc_res)).log2().round() as i64).clamp(0, 24);
    // Serve from z0. The overview chain says where *detail* stops improving,
    // not where tiles stop being available — clamping minzoom to the overview
    // count leaves a client that fits the whole dataset with nothing to show.
    // The real floor is MAX_SOURCE_TILES, which a too-shallow pyramid trips.
    let minzoom = 0;

    let tiles = format!(
        "{}://{}/cog/tiles/{{z}}/{{x}}/{{y}}.png?{}",
        req_url.scheme(),
        req_url
            .host_str()
            .map(|h| match req_url.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            })
            .unwrap_or_default(),
        req_url.query().unwrap_or_default()
    );
    Response::from_json(&serde_json::json!({
        "tilejson": "2.2.0",
        "scheme": "xyz",
        "tiles": [tiles],
        "minzoom": minzoom,
        "maxzoom": maxzoom,
        "bounds": b,
        "center": [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, minzoom],
    }))
    .map(|r| timed(r, &reader))
}

/// `/cog/point/{lon},{lat}` — the raw band values under one coordinate.
pub(crate) async fn point(src: &str, coords: &str) -> Result<Response> {
    let parsed: Vec<f64> = coords
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let [lon, lat] = parsed[..] else {
        return Err(Error::RustError(format!(
            "expected `lon,lat`, got {coords:?}"
        )));
    };

    let (reader, tiff) = open(src).await?;
    let ifds = image_ifds(&tiff);
    let ifd = ifds[0];
    let t = transform_of(ifd)?;
    let crs = source_crs(ifd)?;

    let (wx, wy) = Reproject::between(&Crs::WGS84, &crs)?
        .apply(lon, lat)
        .ok_or_else(|| Error::RustError("point is outside the CRS's domain".into()))?;
    let (fx, fy) = t.world_to_pixel(wx, wy, 1.0);
    let (iw, ih) = (ifd.image_width() as f64, ifd.image_height() as f64);
    if fx < 0.0 || fy < 0.0 || fx >= iw || fy >= ih {
        return Err(Error::RustError("point is outside the image".into()));
    }

    let tw = ifd
        .tile_width()
        .ok_or_else(|| Error::RustError("not a tiled TIFF (stripped COGs unsupported)".into()))?
        as usize;
    let th = ifd.tile_height().unwrap() as usize;
    let (px, py) = (fx as usize, fy as usize);

    let arr = ifd
        .fetch_tile(px / tw, py / th, &reader)
        .await
        .map_err(|e| Error::RustError(format!("tile fetch failed: {e}")))?
        .decode(&decoders())
        .map_err(|e| Error::RustError(format!("decode failed: {e}")))?;
    let nb = arr.shape()[2];
    let (data, _, _) = arr.into_inner();
    let base = ((py % th) * tw + (px % tw)) * nb;

    let nodata = nodata_of(ifd);
    let values: Vec<Option<f64>> = (0..nb)
        .map(|b| {
            let v = sample(&data, base + b);
            (v.is_finite() && nodata != Some(v)).then_some(v)
        })
        .collect();

    Response::from_json(&serde_json::json!({
        "coordinates": [lon, lat],
        "values": values,
        "band_names": (1..=nb).map(|b| format!("b{b}")).collect::<Vec<_>>(),
        "pixel": [px, py],
    }))
}
