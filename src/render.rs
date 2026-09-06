//! Turning decoded samples into an 8-bit RGBA PNG.

use std::collections::HashMap;

use async_tiff::TypedArray;
use worker::*;

use crate::colormap::{Colormap, BUILTIN};
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
pub(crate) fn colormap(q: &HashMap<String, String>) -> Result<Option<Colormap>> {
    if let Some(json) = q.get("colormap").filter(|s| !s.is_empty()) {
        let raw: HashMap<String, Vec<u8>> = serde_json::from_str(json)
            .map_err(|e| Error::RustError(format!("colormap is not valid JSON: {e}")))?;
        let mut table = HashMap::with_capacity(raw.len());
        for (k, v) in raw {
            let key = k.parse::<i64>().map_err(|_| {
                Error::RustError(format!("colormap key {k:?} is not an integer"))
            })?;
            let rgba = match v.len() {
                3 => [v[0], v[1], v[2], 255],
                4 => [v[0], v[1], v[2], v[3]],
                n => {
                    return Err(Error::RustError(format!(
                        "colormap entry {k} has {n} channels, expected 3 or 4"
                    )))
                }
            };
            table.insert(key, rgba);
        }
        return Ok(Some(Colormap::Discrete(table)));
    }

    let Some(name) = q.get("colormap_name").filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    BUILTIN
        .iter()
        .find(|(n, _)| *n == name.to_ascii_lowercase())
        .map(|(_, stops)| Some(Colormap::Continuous(stops)))
        .ok_or_else(|| {
            let known: Vec<&str> = BUILTIN.iter().map(|(n, _)| *n).collect();
            Error::RustError(format!(
                "unknown colormap_name {name:?}; available: {}",
                known.join(", ")
            ))
        })
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
pub(crate) fn resampling(q: &HashMap<String, String>) -> Result<Resampling> {
    match q.get("resampling").filter(|s| !s.is_empty()).map(|s| s.as_str()) {
        None | Some("nearest") => Ok(Resampling::Nearest),
        Some("bilinear") => Ok(Resampling::Bilinear),
        Some(other) => Err(Error::RustError(format!(
            "resampling {other:?} not supported; use nearest or bilinear"
        ))),
    }
}

/// `bidx` is 1-based, like titiler. Defaults to RGB, else the first band.
pub(crate) fn band_indices(q: &HashMap<String, String>, n: usize) -> Result<Vec<usize>> {
    let idx: Vec<usize> = match q.get("bidx").filter(|s| !s.is_empty()) {
        Some(s) => s
            .split(',')
            .map(|p| {
                p.trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|v| (1..=n).contains(v))
                    .map(|v| v - 1)
                    .ok_or_else(|| Error::RustError(format!("bidx {p} out of range 1..={n}")))
            })
            .collect::<Result<_>>()?,
        None if n >= 3 => vec![0, 1, 2],
        None => vec![0],
    };
    match idx.len() {
        1 | 3 => Ok(idx),
        _ => Err(Error::RustError("bidx must name 1 or 3 bands".into())),
    }
}

/// `rescale=min,max` applies to every band; repeat it per band to differ.
pub(crate) fn rescale(q: &HashMap<String, String>, bands: usize) -> Result<(Vec<f64>, Vec<f64>)> {
    let Some(s) = q.get("rescale").filter(|s| !s.is_empty()) else {
        return Ok((vec![0.0; bands], vec![255.0; bands]));
    };
    let pairs: Vec<&str> = s.split(';').collect();
    if pairs.len() != 1 && pairs.len() != bands {
        return Err(Error::RustError(format!(
            "rescale needs 1 or {bands} `min,max` pairs separated by `;`"
        )));
    }
    let (mut lo, mut hi) = (Vec::new(), Vec::new());
    for p in &pairs {
        let v: Vec<f64> = p.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        match v.as_slice() {
            [a, b] if b > a => {
                lo.push(*a);
                hi.push(*b);
            }
            _ => return Err(Error::RustError("rescale must be min,max with max > min".into())),
        }
    }
    if pairs.len() == 1 {
        lo = vec![lo[0]; bands];
        hi = vec![hi[0]; bands];
    }
    Ok((lo, hi))
}

pub(crate) fn png_response(rgba: &[u8]) -> Result<Response> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, TILE as u32, TILE as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| Error::RustError(format!("png: {e}")))?;
        w.write_image_data(rgba)
            .map_err(|e| Error::RustError(format!("png: {e}")))?;
    }
    let mut resp = Response::from_bytes(buf)?;
    resp.headers_mut().set("Content-Type", "image/png")?;
    resp.headers_mut()
        .set("Cache-Control", "public, max-age=3600")?;
    Ok(resp)
}
