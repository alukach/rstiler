//! Everything CRS-shaped: reading the COG's projection, moving Web Mercator
//! coordinates into it, and reporting geographic bounds.

use async_tiff::geo::GeoKeyDirectory;
use async_tiff::ImageFileDirectory;
use crate::fail::{Fail, Out};

use crate::tiling::Transform;
use crate::WEB_MERCATOR;

/// A coordinate reference system, however the GeoTIFF chose to describe it.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Crs {
    /// The ordinary case: an EPSG code, resolved from proj4rs's built-in table.
    Epsg(u16),
    /// GeoKey 32767 — the projection is spelled out in the individual geokeys
    /// rather than named, so we rebuild a proj string from them.
    Proj(String),
}

impl Crs {
    pub(crate) const WGS84: Self = Crs::Epsg(4326);

    pub(crate) fn label(&self) -> String {
        match self {
            Crs::Epsg(c) => format!("EPSG:{c}"),
            Crs::Proj(s) => s.clone(),
        }
    }

    fn proj(&self) -> Out<proj4rs::Proj> {
        match self {
            Crs::Epsg(c) => proj4rs::Proj::from_epsg_code(*c)
                .map_err(|e| Fail::bad(format!("EPSG:{c} unsupported by proj4rs: {e:?}"))),
            Crs::Proj(s) => proj4rs::Proj::from_proj_string(s)
                .map_err(|e| Fail::bad(format!("proj4rs rejected {s:?}: {e:?}"))),
        }
    }
}

/// Look up the COG's CRS from its GeoKeyDirectory.
pub(crate) fn source_crs(ifd: &ImageFileDirectory) -> Out<Crs> {
    let gk = ifd
        .geo_key_directory()
        .ok_or_else(|| Fail::bad("no GeoKeyDirectory: not a GeoTIFF"))?;
    match gk.projected_type.or(gk.geographic_type) {
        Some(32767) | None => Ok(Crs::Proj(proj_string_from_geokeys(gk)?)),
        Some(code) => Ok(Crs::Epsg(code)),
    }
}

/// Rebuild a proj string for a user-defined CRS.
///
/// GeoTIFF spells these out with ProjCoordTransGeoKey naming the projection
/// method and a scatter of parameter geokeys. The parameters come in "natural
/// origin", "false origin" and "center" spellings depending on the method and
/// the writer, so each is looked up in turn.
fn proj_string_from_geokeys(gk: &GeoKeyDirectory) -> Out<String> {
    // A CRS can also name a *coded* projection instead of spelling out a
    // method. 160xx / 161xx are the UTM zones, which is nearly all of them.
    if gk.proj_coord_trans.is_none() {
        if let Some(code) = gk.projection.filter(|c| *c != 32767) {
            let (zone, hemi) = match code {
                16001..=16060 => (code - 16000, "+north"),
                16101..=16160 => (code - 16100, "+south"),
                other => {
                    return Err(Fail::bad(format!(
                        "user-defined CRS names ProjectionGeoKey {other}, which is not implemented"
                    )))
                }
            };
            return Ok(format!(
                "+proj=utm +zone={zone} {hemi} {} {} +no_defs",
                datum_clause(gk),
                units_clause(gk)
            ));
        }
    }

    let method = gk.proj_coord_trans.ok_or_else(|| {
        Fail::bad("user-defined CRS with neither ProjCoordTransGeoKey nor ProjectionGeoKey")
    })?;

    let lat0 = gk
        .proj_false_origin_lat
        .or(gk.proj_nat_origin_lat)
        .or(gk.proj_center_lat)
        .unwrap_or(0.0);
    let lon0 = gk
        .proj_false_origin_long
        .or(gk.proj_nat_origin_long)
        .or(gk.proj_center_long)
        .or(gk.proj_straight_vert_pole_long)
        .unwrap_or(0.0);
    let x0 = gk
        .proj_false_origin_easting
        .or(gk.proj_false_easting)
        .or(gk.proj_center_easting)
        .unwrap_or(0.0);
    let y0 = gk
        .proj_false_origin_northing
        .or(gk.proj_false_northing)
        .or(gk.proj_center_northing)
        .unwrap_or(0.0);
    let k0 = gk.proj_scale_at_nat_origin.or(gk.proj_scale_at_center).unwrap_or(1.0);
    let sp1 = gk.proj_std_parallel1.unwrap_or(lat0);
    let sp2 = gk.proj_std_parallel2.unwrap_or(sp1);

    // ProjCoordTransGeoKey codes, from the GeoTIFF specification.
    let core = match method {
        1 => format!("+proj=tmerc +lat_0={lat0} +lon_0={lon0} +k_0={k0}"),
        7 => format!("+proj=merc +lat_ts={sp1} +lon_0={lon0}"),
        8 => format!("+proj=lcc +lat_0={lat0} +lon_0={lon0} +lat_1={sp1} +lat_2={sp2}"),
        9 => format!("+proj=lcc +lat_0={lat0} +lon_0={lon0} +lat_1={lat0} +lat_2={lat0} +k_0={k0}"),
        10 => format!("+proj=laea +lat_0={lat0} +lon_0={lon0}"),
        11 => format!("+proj=aea +lat_0={lat0} +lon_0={lon0} +lat_1={sp1} +lat_2={sp2}"),
        12 => format!("+proj=aeqd +lat_0={lat0} +lon_0={lon0}"),
        13 => format!("+proj=eqdc +lat_0={lat0} +lon_0={lon0} +lat_1={sp1} +lat_2={sp2}"),
        14 => format!("+proj=stere +lat_0={lat0} +lon_0={lon0} +k_0={k0}"),
        // Polar stereographic: the pole is implied by the sign of the origin.
        15 => format!(
            "+proj=stere +lat_0={} +lat_ts={lat0} +lon_0={lon0}",
            if lat0 < 0.0 { -90 } else { 90 }
        ),
        17 => format!("+proj=eqc +lat_ts={sp1} +lat_0={lat0} +lon_0={lon0}"),
        24 => format!("+proj=sinu +lon_0={lon0}"),
        other => {
            return Err(Fail::bad(format!(
                "user-defined CRS uses ProjCoordTrans {other}, which is not implemented"
            )))
        }
    };

    Ok(format!(
        "{core} +x_0={x0} +y_0={y0} {} {} +no_defs",
        datum_clause(gk),
        units_clause(gk)
    ))
}

/// Prefer a named datum; fall back to the ellipsoid the geokeys describe.
fn datum_clause(gk: &GeoKeyDirectory) -> String {
    match gk.geog_geodetic_datum.or(gk.geographic_type) {
        Some(4326) | Some(6326) => return "+datum=WGS84".into(),
        Some(4269) | Some(6269) => return "+datum=NAD83".into(),
        Some(4267) | Some(6267) => return "+datum=NAD27".into(),
        _ => {}
    }
    match gk.geog_ellipsoid {
        Some(7030) => return "+ellps=WGS84".into(),
        Some(7019) => return "+ellps=GRS80".into(),
        _ => {}
    }
    if let (Some(a), Some(rf)) = (gk.geog_semi_major_axis, gk.geog_inv_flattening) {
        return format!("+a={a} +rf={rf}");
    }
    if let (Some(a), Some(b)) = (gk.geog_semi_major_axis, gk.geog_semi_minor_axis) {
        return format!("+a={a} +b={b}");
    }
    // ponytail: WGS84 is right for almost everything written this way, and the
    // alternative is refusing to serve the file at all.
    "+datum=WGS84".into()
}

fn units_clause(gk: &GeoKeyDirectory) -> String {
    match gk.proj_linear_units {
        Some(9002) => "+units=ft".into(),
        Some(9003) => "+units=us-ft".into(),
        None | Some(9001) => "+units=m".into(),
        // Anything else is described by its size in metres.
        _ => match gk.proj_linear_unit_size {
            Some(m) => format!("+to_meter={m}"),
            None => "+units=m".into(),
        },
    }
}

/// Transforms coordinates between two EPSG-coded CRSs.
pub(crate) struct Reproject {
    from: proj4rs::Proj,
    to: Option<proj4rs::Proj>, // None when source and target are the same
}

impl Reproject {
    pub(crate) fn between(from: &Crs, to: &Crs) -> Out<Self> {
        Ok(Self {
            from: from.proj()?,
            to: (from != to).then(|| to.proj()).transpose()?,
        })
    }

    /// Web Mercator into the COG's own CRS — what serving a tile needs.
    pub(crate) fn new(to: &Crs) -> Out<Self> {
        Self::between(&Crs::Epsg(WEB_MERCATOR), to)
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

pub(crate) fn transform_of(ifd: &ImageFileDirectory) -> Out<Transform> {
    let scale = ifd
        .model_pixel_scale()
        .ok_or_else(|| Fail::bad("no ModelPixelScale: not a north-up GeoTIFF"))?;
    let tp = ifd
        .model_tiepoint()
        .ok_or_else(|| Fail::bad("no ModelTiepoint: not a north-up GeoTIFF"))?;
    Ok(Transform {
        origin_x: tp[3] - tp[0] * scale[0],
        origin_y: tp[4] + tp[1] * scale[1],
        res_x: scale[0],
        res_y: scale[1],
    })
}

/// Corner bounds in EPSG:4326, for TileJSON and for fitting the viewer's map.
pub(crate) fn wgs84_bounds(t: &Transform, crs: &Crs, w: u32, h: u32) -> Out<[f64; 4]> {
    let to_wgs = Reproject::between(crs, &Crs::WGS84)?;
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
