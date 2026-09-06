//! A titiler-shaped dynamic tile server for Cloud Optimized GeoTIFFs, running
//! as a Cloudflare Worker. Reads COGs over HTTP range requests via `async-tiff`,
//! reprojects to Web Mercator with `proj4rs`, and encodes PNG tiles.

mod cog;
mod colormap;
mod geo;
mod meta;
mod render;
mod tiles;
mod tiling;

use std::collections::HashMap;

use worker::*;

use meta::{info, point, statistics, tilejson};
use tiles::tile;

/// Output tile size. titiler defaults to 256; so do we.
pub(crate) const TILE: usize = 256;
/// Refuse to assemble a tile out of more source tiles than this, so a bad
/// request can't fan out into hundreds of range reads.
pub(crate) const MAX_SOURCE_TILES: usize = 64;
/// EPSG code of the tile grid we serve.
pub(crate) const WEB_MERCATOR: u16 = 3857;
/// Source tiles to sample when estimating a display range. Kept small because
/// a COG with no overviews makes these full-resolution reads.
pub(crate) const STATS_TILES: usize = 8;

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().trim_matches('/').to_string();
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();

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

    let Some(src) = q.get("url").cloned() else {
        return Response::error("missing required `url` query parameter", 400);
    };

    let out = match path.split('/').collect::<Vec<_>>().as_slice() {
        ["cog", "info"] => info(&src).await,
        ["cog", "tilejson.json"] => tilejson(&src, &url).await,
        ["cog", "tiles", z, x, y] => tile(&src, z, x, y, &q).await,
        ["cog", "point", coords] => point(&src, coords).await,
        ["cog", "statistics"] => statistics(&src).await,
        _ => return Response::error("not found", 404),
    };

    match out {
        Ok(resp) => Ok(resp),
        // JSON, so the viewer can show the message rather than a bare status.
        Err(e) => Response::from_json(&serde_json::json!({ "detail": format!("{e}") }))
            .map(|r| r.with_status(500)),
    }
}
