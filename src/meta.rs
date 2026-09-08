//! The metadata endpoints: `/cog/info` and `/cog/tilejson.json`.

use async_tiff::ImageFileDirectory;
use worker::*;

use crate::cog::{decoders, nodata_of, Cog, HttpReader};
use crate::fail::{Fail, Out};
use crate::query::Query;
use crate::render::band_indices;
use crate::geo::{source_crs, transform_of, wgs84_bounds, Crs, Reproject};
use crate::render::sample;
use crate::{tiling, STATS_TILES, TILE};

pub(crate) async fn info(src: &str, _q: &Query) -> Out<Response> {
    let cog = Cog::open(src).await?;
    let ifds = cog.levels();
    let ifd = &ifds[0];
    let t = transform_of(ifd)?;
    let crs = source_crs(ifd)?;
    let bands = ifd.samples_per_pixel() as usize;

    // Field names follow rio_tiler.models.Info, which is what a titiler client
    // parses. Everything past `colorinterp` is ours; the model allows extras.
    let names: Vec<String> = (1..=bands).map(|b| format!("b{b}")).collect();
    Response::from_json(&serde_json::json!({
        "bounds": wgs84_bounds(&t, &crs, ifd.image_width(), ifd.image_height())?,
        "band_metadata": names.iter().map(|n| (n, serde_json::Map::new())).collect::<Vec<_>>(),
        "band_descriptions": names.iter().map(|n| (n, n)).collect::<Vec<_>>(),
        "dtype": dtype(ifd),
        "nodata_type": nodata_type(ifd),
        "colorinterp": colorinterp(ifd),

        "width": ifd.image_width(),
        "height": ifd.image_height(),
        "band_count": bands,
        "bits_per_sample": ifd.bits_per_sample(),
        "compression": format!("{:?}", ifd.compression()),
        "nodata": ifd.gdal_nodata(),
        "crs": crs.label(),
        "overviews": ifds.len() - 1,
        "resolution": [t.res_x, t.res_y],
    }))
    .map_err(Fail::from)
    .map(|r| timed(r, &cog.reader))
}

/// The numpy-style name for a band's samples, as rio-tiler reports it.
fn dtype(ifd: &ImageFileDirectory) -> String {
    let bits = ifd.bits_per_sample().first().copied().unwrap_or(8);
    let fmt = ifd
        .sample_format()
        .first()
        .map(|f| format!("{f:?}"))
        .unwrap_or_default();
    match fmt.as_str() {
        "Int" => format!("int{bits}"),
        "IEEEFP" => format!("float{bits}"),
        _ => format!("uint{bits}"),
    }
}

/// Which of rio-tiler's masking mechanisms this dataset uses.
fn nodata_type(ifd: &ImageFileDirectory) -> &'static str {
    if ifd.gdal_nodata().is_some() {
        "Nodata"
    } else if ifd
        .extra_samples()
        .is_some_and(|e| !e.is_empty())
    {
        "Alpha"
    } else {
        "None"
    }
}

fn colorinterp(ifd: &ImageFileDirectory) -> Vec<&'static str> {
    let n = ifd.samples_per_pixel() as usize;
    let mut out: Vec<&str> = if n >= 3 {
        vec!["red", "green", "blue"]
    } else {
        vec!["gray"]
    };
    while out.len() < n {
        out.push("alpha");
    }
    out.truncate(n);
    out
}

/// Attaches the range-request accounting that explains a response's latency.
fn timed(mut resp: Response, reader: &HttpReader) -> Response {
    let (reqs, hits, bytes) = reader.traffic();
    let _ = resp.headers_mut().set(
        "Server-Timing",
        &format!(
            "origin;desc=\"{} range reads, {hits} cached, {} KiB\"",
            reqs - hits,
            bytes / 1024
        ),
    );
    resp
}

/// `/cog/statistics` — per-band summary, read off the coarsest overview.
///
/// Split out of `/cog/info` deliberately: a COG with no overview pyramid makes
/// this a full-resolution read, and origins charge ~2 s per range request. Keep
/// `/cog/info` to metadata a client always needs.
pub(crate) async fn statistics(src: &str, _q: &Query) -> Out<Response> {
    let cog = Cog::open(src).await?;
    let bands = cog.levels()[0].samples_per_pixel() as usize;
    let nodata = nodata_of(&cog.levels()[0]);
    // Sample the coarsest level: fewest tiles, and its arrays are the smallest
    // to read.
    let coarse = cog.full(cog.coarsest()).await?;
    let stats = percentiles(&coarse.ifds()[0], &cog.reader, nodata, bands).await?;

    let body: serde_json::Map<String, serde_json::Value> = stats
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("b{}", i + 1), serde_json::to_value(s).unwrap()))
        .collect();
    Response::from_json(&body)
        .map_err(Fail::from)
        .map(|r| timed(r, &cog.reader))
}

/// Per-band summary, computed from a sample of one overview level.
///
/// Field names follow rio_tiler.models.BandStatistics so a titiler client can
/// parse this. `histogram`, `majority`, `minority` and `unique` are not here —
/// see the conformance suite.
#[derive(serde::Serialize)]
pub(crate) struct BandStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub count: f64,
    pub sum: f64,
    pub std: f64,
    pub median: f64,
    pub valid_percent: f64,
    pub masked_pixels: f64,
    pub valid_pixels: f64,
    pub description: String,
    pub percentile_2: f64,
    pub percentile_98: f64,
}

async fn percentiles(
    ifd: &ImageFileDirectory,
    reader: &HttpReader,
    nodata: Option<f64>,
    bands: usize,
) -> Out<Vec<BandStats>> {
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
        .map_err(|e| Fail::upstream(format!("stats fetch failed: {e}")))?;

    let mut per_band: Vec<Vec<f64>> = vec![Vec::new(); bands];
    let mut examined = 0usize;
    for t in tiles {
        let arr = t
            .decode(&decoders())
            .map_err(|e| Fail::bad(format!("stats decode failed: {e}")))?;
        let nb = arr.shape()[2];
        let (data, _, _) = arr.into_inner();
        // One value in every 16 pixels is plenty for a percentile, and keeps
        // the sort off the hot path on a full-resolution tile.
        for i in (0..data.len()).step_by(nb * 16) {
            examined += 1;
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
        .enumerate()
        .map(|(b, mut v)| {
            let description = format!("b{}", b + 1);
            let masked = (examined - v.len()) as f64;
            if v.is_empty() {
                return BandStats {
                    min: 0.0,
                    max: 0.0,
                    mean: 0.0,
                    count: 0.0,
                    sum: 0.0,
                    std: 0.0,
                    median: 0.0,
                    valid_percent: 0.0,
                    masked_pixels: masked,
                    valid_pixels: 0.0,
                    description,
                    percentile_2: 0.0,
                    percentile_98: 255.0,
                };
            }
            v.sort_by(|a, b| a.total_cmp(b));
            let n = v.len() as f64;
            let sum: f64 = v.iter().sum();
            let mean = sum / n;
            let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
            let at = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
            let (lo, hi) = (at(0.02), at(0.98));
            BandStats {
                min: v[0],
                max: v[v.len() - 1],
                mean,
                count: n,
                sum,
                std: var.sqrt(),
                median: at(0.5),
                valid_percent: if examined == 0 { 0.0 } else { n / examined as f64 * 100.0 },
                masked_pixels: masked,
                valid_pixels: n,
                percentile_2: lo,
                // Keep the range non-empty so it is always usable as a rescale.
                percentile_98: if hi > lo { hi } else { lo + 1.0 },
                description,
            }
        })
        .collect())
}

pub(crate) async fn tilejson(src: &str, req_url: &Url, q: &Query) -> Out<Response> {
    let cog = Cog::open(src).await?;
    let ifds = cog.levels();
    let ifd = &ifds[0];
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
    let mut maxzoom =
        ((2.0 * tiling::R / (TILE as f64 * merc_res)).log2().round() as i64).clamp(0, 24);
    // Serve from z0. The overview chain says where *detail* stops improving,
    // not where tiles stop being available — clamping minzoom to the overview
    // count leaves a client that fits the whole dataset with nothing to show.
    // The real floor is MAX_SOURCE_TILES, which a too-shallow pyramid trips.
    let mut minzoom = 0;

    // titiler lets a request pin the zoom range it wants advertised.
    for (key, slot) in [("minzoom", &mut minzoom), ("maxzoom", &mut maxzoom)] {
        if let Some(v) = q.last(key) {
            *slot = v
                .parse()
                .map_err(|_| Fail::bad(format!("{key} {v:?} is not an integer")))?;
        }
    }

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
    .map_err(Fail::from)
    .map(|r| timed(r, &cog.reader))
}

/// `/cog/point/{lon},{lat}` — the raw band values under one coordinate.
pub(crate) async fn point(src: &str, coords: &str, q: &Query) -> Out<Response> {
    let parsed: Vec<f64> = coords
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let [lon, lat] = parsed[..] else {
        return Err(Fail::bad(format!("expected `lon,lat`, got {coords:?}")));
    };

    let cog = Cog::open(src).await?;
    let ifds = cog.levels();
    let ifd = &ifds[0];
    let t = transform_of(ifd)?;
    let crs = source_crs(ifd)?;

    let (wx, wy) = Reproject::between(&Crs::WGS84, &crs)?
        .apply(lon, lat)
        .ok_or_else(|| Fail::bad("point is outside the CRS's domain"))?;
    let (fx, fy) = t.world_to_pixel(wx, wy, 1.0);
    let (iw, ih) = (ifd.image_width() as f64, ifd.image_height() as f64);
    if fx < 0.0 || fy < 0.0 || fx >= iw || fy >= ih {
        return Err(Fail::bad("point is outside the image"));
    }

    // Only now do we need level 0's per-tile arrays.
    let full = cog.full(0).await?;
    let ifd = &full.ifds()[0];
    let tw = ifd
        .tile_width()
        .ok_or_else(|| Fail::bad("not a tiled TIFF (stripped COGs unsupported)"))?
        as usize;
    let th = ifd.tile_height().unwrap() as usize;
    let (px, py) = (fx as usize, fy as usize);

    let arr = ifd
        .fetch_tile(px / tw, py / th, &cog.reader)
        .await
        .map_err(|e| Fail::upstream(format!("tile fetch failed: {e}")))?
        .decode(&decoders())
        .map_err(|e| Fail::bad(format!("decode failed: {e}")))?;
    let nb = arr.shape()[2];
    let (data, _, _) = arr.into_inner();
    let base = ((py % th) * tw + (px % tw)) * nb;

    // titiler lets a request override the dataset's nodata here too.
    let nodata = match q.last("nodata") {
        Some(s) => Some(
            s.parse::<f64>()
                .map_err(|_| Fail::bad(format!("nodata {s:?} is not a number")))?,
        ),
        None => nodata_of(ifd),
    };

    // Bands follow bidx, repeats and all; band_names name what was asked for.
    let bands = band_indices(q, nb)?;
    let values: Vec<Option<f64>> = bands
        .iter()
        .map(|b| {
            let v = sample(&data, base + b);
            (v.is_finite() && nodata != Some(v)).then_some(v)
        })
        .collect();

    Response::from_json(&serde_json::json!({
        "coordinates": [lon, lat],
        "values": values,
        "band_names": bands.iter().map(|b| format!("b{}", b + 1)).collect::<Vec<_>>(),
        "pixel": [px, py],
    }))
    .map_err(Fail::from)
}
