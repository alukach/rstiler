//! The scraps of `GDAL_METADATA` we need.
//!
//! GDAL stores per-band scale and offset in an XML blob in tag 42112 rather
//! than in TIFF fields, so `unscale=` has to read it out:
//!
//! ```xml
//! <GDALMetadata>
//!   <Item name="OFFSET" sample="0" role="offset">100</Item>
//!   <Item name="SCALE" sample="0" role="scale">0.01</Item>
//! </GDALMetadata>
//! ```
//!
//! This is deliberately not a general XML parser. It looks for `<Item>`
//! elements, reads their attributes, and ignores everything else — enough for
//! the two roles that change pixel values, and small enough to be obviously
//! correct.
//!
//! Dependency-free, so: `rustc --test src/gdalmeta.rs -o /tmp/g && /tmp/g`

/// Scale and offset for one band, as `value * scale + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scaling {
    pub scale: f64,
    pub offset: f64,
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: 0.0,
        }
    }
}

impl Scaling {
    pub(crate) fn apply(&self, v: f64) -> f64 {
        v * self.scale + self.offset
    }

    /// True when applying this would not change anything.
    pub(crate) fn is_identity(&self) -> bool {
        self.scale == 1.0 && self.offset == 0.0
    }
}

/// Per-band scaling, indexed 0-based, for `bands` bands.
pub(crate) fn scaling(xml: Option<&str>, bands: usize) -> Vec<Scaling> {
    let mut out = vec![Scaling::default(); bands];
    let Some(xml) = xml else { return out };

    for item in items(xml) {
        let Some(role) = item.attr("role") else {
            continue;
        };
        let Ok(value) = item.text.trim().parse::<f64>() else {
            continue;
        };
        // `sample` is the band, 0-based, and defaults to the first.
        let band: usize = item
            .attr("sample")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let Some(slot) = out.get_mut(band) else {
            continue;
        };
        match role {
            "scale" => slot.scale = value,
            "offset" => slot.offset = value,
            _ => {}
        }
    }
    out
}

struct Item<'a> {
    attrs: &'a str,
    text: &'a str,
}

impl Item<'_> {
    /// Value of `name="..."`, without unescaping — these are numbers and
    /// short identifiers, never entities.
    fn attr(&self, name: &str) -> Option<&str> {
        let needle = format!("{name}=\"");
        let at = self.attrs.find(&needle)? + needle.len();
        let rest = &self.attrs[at..];
        Some(&rest[..rest.find('"')?])
    }
}

fn items(xml: &str) -> Vec<Item<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Item") {
        rest = &rest[start + 5..];
        let Some(gt) = rest.find('>') else { break };
        let attrs = &rest[..gt];
        rest = &rest[gt + 1..];
        // A self-closing item carries no value, so nothing to read.
        if attrs.ends_with('/') {
            continue;
        }
        let Some(end) = rest.find("</Item>") else {
            break;
        };
        out.push(Item {
            attrs,
            text: &rest[..end],
        });
        rest = &rest[end + 7..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GDAL: &str = r#"<GDALMetadata>
  <Item name="OFFSET" sample="0" role="offset">100</Item>
  <Item name="SCALE" sample="0" role="scale">0.01</Item>
  <Item name="OVERVIEW_RESAMPLING" domain="IMAGE_STRUCTURE">BILINEAR</Item>
</GDALMetadata>"#;

    #[test]
    fn reads_scale_and_offset() {
        let s = scaling(Some(GDAL), 1);
        assert_eq!(s[0].scale, 0.01);
        assert_eq!(s[0].offset, 100.0);
        // 20000 raw becomes 300 real: the fixture's own numbers.
        assert!((s[0].apply(20000.0) - 300.0).abs() < 1e-9);
    }

    #[test]
    fn items_without_a_role_are_ignored() {
        // OVERVIEW_RESAMPLING has no role and is not a number; it must not
        // upset anything.
        assert_eq!(scaling(Some(GDAL), 1).len(), 1);
    }

    #[test]
    fn per_band_sample_index() {
        let xml = r#"<GDALMetadata>
          <Item sample="0" role="scale">2</Item>
          <Item sample="2" role="scale">5</Item>
        </GDALMetadata>"#;
        let s = scaling(Some(xml), 3);
        assert_eq!(s[0].scale, 2.0);
        assert_eq!(s[1].scale, 1.0, "untouched bands stay identity");
        assert_eq!(s[2].scale, 5.0);
    }

    #[test]
    fn a_sample_beyond_the_band_count_is_dropped() {
        let xml = r#"<Item sample="9" role="scale">2</Item>"#;
        assert_eq!(scaling(Some(xml), 2), vec![Scaling::default(); 2]);
    }

    #[test]
    fn absent_or_empty_metadata_is_identity() {
        assert!(scaling(None, 2).iter().all(Scaling::is_identity));
        assert!(scaling(Some(""), 2).iter().all(Scaling::is_identity));
        assert!(scaling(Some("<GDALMetadata/>"), 1)
            .iter()
            .all(Scaling::is_identity));
    }

    #[test]
    fn self_closing_items_do_not_derail_the_scan() {
        let xml = r#"<Item name="x" role="scale"/><Item sample="0" role="scale">3</Item>"#;
        assert_eq!(scaling(Some(xml), 1)[0].scale, 3.0);
    }
}
