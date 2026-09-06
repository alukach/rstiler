//! Everything CRS-shaped: reading the COG's projection, moving Web Mercator
//! coordinates into it, and reporting geographic bounds.

use async_tiff::ImageFileDirectory;
use worker::*;

use crate::tiling::Transform;
use crate::WEB_MERCATOR;

/// Look up the COG's CRS. GeoTIFF stores it as an EPSG code in one of two
/// geokeys; 32767 means "user-defined", which needs the projection rebuilt
/// from the individual geokeys.
pub(crate) fn source_crs(ifd: &ImageFileDirectory) -> Result<u16> {
    let gk = ifd
        .geo_key_directory()
        .ok_or_else(|| Error::RustError("no GeoKeyDirectory: not a GeoTIFF".into()))?;
    match gk.projected_type.or(gk.geographic_type) {
        Some(32767) | None => Err(Error::RustError(
            // ponytail: rebuilding a proj string from proj_coord_trans and
            // friends means a case per projection family. Add it when a
            // user-defined CRS actually needs serving.
            "user-defined CRS (geokey 32767); only EPSG-coded CRSs are supported".into(),
        )),
        Some(code) => Ok(code),
    }
}

/// Transforms coordinates between two EPSG-coded CRSs.
pub(crate) struct Reproject {
    from: proj4rs::Proj,
    to: Option<proj4rs::Proj>, // None when source and target are the same
}

fn proj(code: u16) -> Result<proj4rs::Proj> {
    proj4rs::Proj::from_epsg_code(code)
        .map_err(|e| Error::RustError(format!("EPSG:{code} unsupported by proj4rs: {e:?}")))
}

impl Reproject {
    pub(crate) fn between(from: u16, to: u16) -> Result<Self> {
        Ok(Self {
            from: proj(from)?,
            to: (from != to).then(|| proj(to)).transpose()?,
        })
    }

    /// Web Mercator into the COG's own CRS — what serving a tile needs.
    pub(crate) fn new(code: u16) -> Result<Self> {
        Self::between(WEB_MERCATOR, code)
    }

    /// `None` when the point falls outside the target projection's domain.
    ///
    /// proj4rs works in radians for geographic CRSs, while GeoTIFF corners and
    /// `lon,lat` query parameters are both in degrees, so convert on each end.
    pub(crate) fn apply(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let Some(to) = &self.to else {
            return Some((x, y));
        };
        let mut p = if self.from.is_latlong() {
            (x.to_radians(), y.to_radians(), 0.0)
        } else {
            (x, y, 0.0)
        };
        proj4rs::transform::transform(&self.from, to, &mut p).ok()?;
        Some(if to.is_latlong() {
            (p.0.to_degrees(), p.1.to_degrees())
        } else {
            (p.0, p.1)
        })
    }
}

pub(crate) fn transform_of(ifd: &ImageFileDirectory) -> Result<Transform> {
    let scale = ifd
        .model_pixel_scale()
        .ok_or_else(|| Error::RustError("no ModelPixelScale: not a north-up GeoTIFF".into()))?;
    let tp = ifd
        .model_tiepoint()
        .ok_or_else(|| Error::RustError("no ModelTiepoint: not a north-up GeoTIFF".into()))?;
    Ok(Transform {
        origin_x: tp[3] - tp[0] * scale[0],
        origin_y: tp[4] + tp[1] * scale[1],
        res_x: scale[0],
        res_y: scale[1],
    })
}

/// Corner bounds in EPSG:4326, for TileJSON and for fitting the viewer's map.
pub(crate) fn wgs84_bounds(t: &Transform, code: u16, w: u32, h: u32) -> Result<[f64; 4]> {
    let to_wgs = Reproject::between(code, 4326)?;
    let (x1, y1) = (
        t.origin_x + w as f64 * t.res_x,
        t.origin_y - h as f64 * t.res_y,
    );
    // Walk the edges, not just the corners: a projected footprint has curved sides.
    let (mut w_, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..=8 {
        let f = i as f64 / 8.0;
        let (mx, my) = (t.origin_x + f * (x1 - t.origin_x), t.origin_y + f * (y1 - t.origin_y));
        for (px, py) in [(mx, t.origin_y), (mx, y1), (t.origin_x, my), (x1, my)] {
            if let Some((lon, lat)) = to_wgs.apply(px, py) {
                (w_, s, e, n) = (w_.min(lon), s.min(lat), e.max(lon), n.max(lat));
            }
        }
    }
    Ok([w_, s, e, n])
}
