//! Turning decoded samples into an 8-bit RGBA PNG.

use async_tiff::TypedArray;
use worker::*;

use std::collections::HashMap;

use crate::colormap::{Colormap, BUILTIN};
use crate::fail::{Fail, Out};
use crate::query::Query;
use crate::TILE;

pub(crate) fn sample(data: &TypedArray, i: usize) -> f64 {
    match data {
        TypedArray::Bool(v) => v[i] as u8 as f64,
        TypedArray::UInt8(v) => v[i] as f64,
        TypedArray::UInt16(v) => v[i] as f64,
        TypedArray::UInt32(v) => v[i] as f64,
        TypedArray::UInt64(v) => v[i] as f64,
        TypedArray::Int8(v) => v[i] as f64,
        TypedArray::Int16(v) => v[i] as f64,
        TypedArray::Int32(v) => v[i] as f64,
        TypedArray::Int64(v) => v[i] as f64,
        TypedArray::Float32(v) => v[i] as f64,
        TypedArray::Float64(v) => v[i],
    }
}

/// Reads titiler's two spellings. Returns `None` when neither is given.
pub(crate) fn colormap(q: &Query) -> Out<Option<Colormap>> {
    if let Some(json) = q.last("colormap") {
        let raw: HashMap<String, serde_json::Value> = serde_json::from_str(json)
            .map_err(|e| Fail::bad(format!("colormap is not valid JSON: {e}")))?;
        let mut table = HashMap::with_capacity(raw.len());
        for (k, v) in raw {
            let key = k
                .parse::<i64>()
                .map_err(|_| Fail::bad(format!("colormap key {k:?} is not an integer")))?;
            table.insert(
                key,
                parse_colour(&v).map_err(|e| Fail::bad(format!("colormap entry {k}: {e}")))?,
            );
        }
        return Ok(Some(Colormap::Discrete(table)));
    }

    let Some(name) = q.last("colormap_name") else {
        return Ok(None);
    };
    BUILTIN
        .iter()
        .find(|(n, _)| *n == name.to_ascii_lowercase())
        .map(|(_, stops)| Some(Colormap::Continuous(stops)))
        .ok_or_else(|| {
            let known: Vec<&str> = BUILTIN.iter().map(|(n, _)| *n).collect();
            Fail::bad(format!(
                "unknown colormap_name {name:?}; available: {}",
                known.join(", ")
            ))
        })
}

/// titiler accepts a colour as `[r,g,b]`, `[r,g,b,a]`, `"#rrggbb"` or
/// `"#rrggbbaa"`. Alpha defaults to opaque.
fn parse_colour(v: &serde_json::Value) -> std::result::Result<[u8; 4], String> {
    if let Some(hex) = v.as_str() {
        let h = hex.strip_prefix('#').unwrap_or(hex);
        if h.len() != 6 && h.len() != 8 {
            return Err(format!("{hex:?} is not #rrggbb or #rrggbbaa"));
        }
        let byte = |i: usize| {
            u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("{hex:?} is not hex"))
        };
        return Ok([
            byte(0)?,
            byte(2)?,
            byte(4)?,
            if h.len() == 8 { byte(6)? } else { 255 },
        ]);
    }
    let arr = v
        .as_array()
        .ok_or("expected [r,g,b], [r,g,b,a] or \"#rrggbb\"")?;
    let chan = |i: usize| -> std::result::Result<u8, String> {
        arr[i]
            .as_u64()
            .filter(|n| *n <= 255)
            .map(|n| n as u8)
            .ok_or_else(|| format!("channel {i} is not 0-255"))
    };
    match arr.len() {
        3 => Ok([chan(0)?, chan(1)?, chan(2)?, 255]),
        4 => Ok([chan(0)?, chan(1)?, chan(2)?, chan(3)?]),
        n => Err(format!("{n} channels, expected 3 or 4")),
    }
}

/// How a source pixel is picked for an output pixel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Resampling {
    Nearest,
    Bilinear,
}

impl Resampling {
    /// `get` returns a source sample, or `None` for nodata / out of range.
    /// Bilinear treats any nodata neighbour as making the whole pixel invalid,
    /// so masked edges stay crisp instead of bleeding into the data.
    pub(crate) fn sample(
        self,
        fx: f64,
        fy: f64,
        iw: f64,
        ih: f64,
        get: impl Fn(usize, usize) -> Option<f64>,
    ) -> Option<f64> {
        if self == Self::Nearest {
            return get(fx as usize, fy as usize);
        }
        // Sample centres sit at pixel centres, so shift by half a pixel.
        let (gx, gy) = (fx - 0.5, fy - 0.5);
        let (x0, y0) = (gx.floor(), gy.floor());
        let (dx, dy) = (gx - x0, gy - y0);
        let cx = |v: f64| v.clamp(0.0, iw - 1.0) as usize;
        let cy = |v: f64| v.clamp(0.0, ih - 1.0) as usize;
        let (xa, xb) = (cx(x0), cx(x0 + 1.0));
        let (ya, yb) = (cy(y0), cy(y0 + 1.0));
        let (p00, p10) = (get(xa, ya)?, get(xb, ya)?);
        let (p01, p11) = (get(xa, yb)?, get(xb, yb)?);
        let top = p00 + dx * (p10 - p00);
        let bottom = p01 + dx * (p11 - p01);
        Some(top + dy * (bottom - top))
    }
}

/// `resampling=nearest|bilinear`, defaulting to nearest as titiler does.
pub(crate) fn resampling(q: &Query) -> Out<Resampling> {
    match q.last("resampling") {
        None | Some("nearest") => Ok(Resampling::Nearest),
        Some("bilinear") => Ok(Resampling::Bilinear),
        Some(other) => Err(Fail::bad(format!(
            "resampling {other:?} not supported; use nearest or bilinear"
        ))),
    }
}

/// `bidx` is 1-based, like titiler. Defaults to RGB, else the first band.
pub(crate) fn band_indices(q: &Query, n: usize) -> Out<Vec<usize>> {
    let given = q.all("bidx");
    if given.is_empty() {
        // titiler's default: RGB when there are three or more bands.
        return Ok(if n >= 3 { vec![0, 1, 2] } else { vec![0] });
    }
    // Repeats are meaningful — titiler's own test asks for b1 three times to
    // render a single band as grey RGB.
    // Any number is valid here — /cog/point reports one value per entry.
    // Rendering a *tile* needs 1 or 3, which the tile handler enforces.
    given
        .iter()
        .map(|p| {
            p.parse::<usize>()
                .ok()
                .filter(|v| (1..=n).contains(v))
                .map(|v| v - 1)
                .ok_or_else(|| Fail::bad(format!("bidx {p} out of range 1..={n}")))
        })
        .collect()
}

/// `rescale=min,max` applies to every band; repeat it per band to differ.
pub(crate) fn rescale(q: &Query, bands: usize) -> Out<(Vec<f64>, Vec<f64>)> {
    // titiler repeats the parameter for per-band ranges; `;` does the same here.
    let given: Vec<&str> = q
        .whole("rescale")
        .iter()
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    if given.is_empty() {
        return Ok((vec![0.0; bands], vec![255.0; bands]));
    }
    if given.len() != 1 && given.len() != bands {
        return Err(Fail::bad(format!(
            "rescale needs 1 or {bands} `min,max` pairs"
        )));
    }
    let (mut lo, mut hi) = (Vec::new(), Vec::new());
    for p in &given {
        let v: Vec<f64> = p.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        match v.as_slice() {
            [a, b] if b > a => {
                lo.push(*a);
                hi.push(*b);
            }
            _ => {
                return Err(Fail::bad(format!(
                    "rescale {p:?} must be min,max with max > min"
                )))
            }
        }
    }
    if given.len() == 1 {
        lo = vec![lo[0]; bands];
        hi = vec![hi[0]; bands];
    }
    Ok((lo, hi))
}

pub(crate) fn png_response(rgba: &[u8]) -> Out<Response> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, TILE as u32, TILE as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| Fail::from(Error::RustError(format!("png: {e}"))))?;
        w.write_image_data(rgba)
            .map_err(|e| Fail::from(Error::RustError(format!("png: {e}"))))?;
    }
    let mut resp = Response::from_bytes(buf)?;
    resp.headers_mut().set("Content-Type", "image/png")?;
    resp.headers_mut()
        .set("Cache-Control", "public, max-age=3600")?;
    Ok(resp)
}
