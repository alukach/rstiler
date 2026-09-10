//! A titiler-shaped dynamic tile server for Cloud Optimized GeoTIFFs, running
//! as a Cloudflare Worker. Reads COGs over HTTP range requests via `async-tiff`,
//! reprojects to Web Mercator with `proj4rs`, and encodes PNG tiles.

mod cog;
mod colormap;
mod expression;
mod fail;
mod gdalmeta;
mod geo;
mod lazyifd;
mod meta;
mod query;
mod render;
mod tiles;
mod tiling;
mod warp;

use worker::*;

use fail::{Fail, Out};
use query::Query;

/// Parameters every endpoint takes.
const COMMON: &[&str] = &["url"];
/// Parameters that shape a rendered tile.
const RENDER: &[&str] = &[
    "url",
    "bidx",
    "expression",
    "rescale",
    "colormap",
    "colormap_name",
    "resampling",
    "nodata",
    "unscale",
    "return_mask",
    "tilesize",
];
/// What `/cog/tilejson.json` accepts: it copies the query into the tile URLs.
const TILEJSON: &[&str] = &[
    "url",
    "bidx",
    "expression",
    "rescale",
    "colormap",
    "colormap_name",
    "resampling",
    "nodata",
    "unscale",
    "return_mask",
    "tilesize",
    "minzoom",
    "maxzoom",
];
/// What `/cog/point` accepts.
const POINT: &[&str] = &["url", "bidx", "nodata", "coord_crs", "unscale"];
/// titiler parameters this server does not implement. Named separately so the
/// error can say "not implemented" rather than "unknown", which is the
/// difference between a gap and a typo.
const UNIMPLEMENTED: &[&str] = &[
    "color_formula",
    "buffer",
    "padding",
    "algorithm",
    "algorithm_params",
    "dst_crs",
    "tileMatrixSetId",
    "max_size",
    "width",
    "height",
    "format",
];

use meta::{info, point, statistics, tilejson};
use tiles::tile;

/// Output tile size. titiler defaults to 256; so do we.
pub(crate) const TILE: usize = 256;
/// Refuse to assemble a tile out of more source tiles than this. Each one is a
/// subrequest, and Workers caps those per request (1000 paid, 50 free), so the
/// ceiling is the platform's, not an arbitrary one. A COG whose overview
/// pyramid stops well short of the zoom being asked for genuinely needs this
/// many reads — the whole coarsest level is often only a couple of hundred
/// tiles, so this is bounded, not runaway.
pub(crate) const MAX_SOURCE_TILES: usize = 256;
/// EPSG code of the tile grid we serve.
pub(crate) const WEB_MERCATOR: u16 = 3857;
/// Source tiles to sample when estimating a display range. Kept small because
/// a COG with no overviews makes these full-resolution reads.
pub(crate) const STATS_TILES: usize = 8;

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().trim_matches('/').to_string();
    let q = Query::new(url.query_pairs().into_owned());

    // `/cog/viewer` is where titiler puts its viewer too. `/` forwards there,
    // keeping the query so a shared `?url=...` link still lands on its COG.
    if path.is_empty() {
        let mut to = url.clone();
        to.set_path("/cog/viewer");
        return Response::redirect_with_status(to, 302);
    }
    if path == "cog/viewer" {
        return Response::from_html(include_str!("viewer.html"));
    }

    let parts: Vec<&str> = path.split('/').collect();

    // A rendered tile is a pure function of its URL, and rendering one costs a
    // metadata parse even when every source byte is already cached. Cache the
    // PNG itself so a second viewer of the same tile pays neither.
    // Keyed by version so a release that changes rendering does not serve
    // tiles drawn by the previous one.
    // ponytail: bump the crate version when you change how pixels are made, or
    // pass a cache-buster (fixtures/check.py does) while iterating locally.
    let is_tile = matches!(parts.as_slice(), ["cog", "tiles", ..]);
    let cache = Cache::default();
    let key = format!("{url}#v{}", env!("CARGO_PKG_VERSION"));
    // Standard HTTP: a client asking for no-cache gets a fresh render. This is
    // also how the test harness avoids grading a previous build's pixels,
    // without a bypass parameter that would itself have to be allowed through.
    let fresh = req
        .headers()
        .get("Cache-Control")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_ascii_lowercase().contains("no-cache"));
    if is_tile && !fresh {
        if let Ok(Some(hit)) = cache.get(&key, true).await {
            return Ok(hit);
        }
    }

    let out = route(&parts, &url, &q).await;

    match out {
        Ok(mut resp) => {
            if is_tile {
                if let Ok(copy) = resp.cloned() {
                    let _ = cache.put(&key, copy).await;
                }
            }
            Ok(resp)
        }
        // JSON with the detail, so the viewer can show what went wrong. The
        // status is the part clients branch on.
        Err(f) => Response::from_json(&serde_json::json!({ "detail": f.detail }))
            .map(|r| r.with_status(f.status)),
    }
}

async fn route(parts: &[&str], url: &Url, q: &Query) -> Out<Response> {
    // Every route needs a source, and titiler treats a missing required query
    // parameter as a client error.
    let src = q
        .last("url")
        .ok_or_else(|| Fail::bad("missing required `url` query parameter"))?
        .to_string();

    // Refuse a parameter we do not honour rather than ignore it. A client that
    // asked for something and got a tile anyway cannot tell it was dropped.
    let known: &[&str] = match parts {
        ["cog", "tiles", ..] => RENDER,
        // tilejson copies the query into the tile URLs it hands out, so it has
        // to accept everything a tile does.
        ["cog", "tilejson.json"] => TILEJSON,
        ["cog", "point", _] => POINT,
        _ => COMMON,
    };
    q.reject_unknown(known, UNIMPLEMENTED).map_err(Fail::bad)?;

    match parts {
        ["cog", "info"] => info(&src, q).await,
        ["cog", "tilejson.json"] => tilejson(&src, url, q).await,
        ["cog", "tiles", z, x, y] => tile(&src, z, x, y, q).await,
        ["cog", "point", coords] => point(&src, coords, q).await,
        ["cog", "statistics"] => statistics(&src, q).await,
        _ => Err(Fail::not_found(format!(
            "no route for /{}",
            parts.join("/")
        ))),
    }
}
