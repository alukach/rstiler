//! Failures that carry the status code they should be reported with.
//!
//! titiler distinguishes a bad request from a broken one — a malformed
//! `colormap` is a 400, a missing dataset a 404 — and clients rely on it.
//! Everything used to be a 500 here.

/// A failed request, and what to tell the client.
pub(crate) struct Fail {
    pub(crate) status: u16,
    pub(crate) detail: String,
}

impl Fail {
    /// The request was wrong: a bad parameter, an unparseable value.
    pub(crate) fn bad(detail: impl Into<String>) -> Self {
        Self {
            status: 400,
            detail: detail.into(),
        }
    }

    pub(crate) fn not_found(detail: impl Into<String>) -> Self {
        Self {
            status: 404,
            detail: detail.into(),
        }
    }

    /// We could not read the COG. If the origin said something specific about
    /// why, pass that status along rather than flattening it to a 500: a
    /// missing or forbidden source is the caller's problem, not ours.
    pub(crate) fn upstream(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let status = match upstream_status(&detail) {
            Some(404) => 404,
            Some(403) | Some(401) => 403,
            Some(s) if (400..500).contains(&s) => 400,
            // A 5xx from the origin, or a transport error, is a bad gateway.
            _ => 502,
        };
        Self { status, detail }
    }
}

/// Recover the origin's status from a reader error.
///
/// ponytail: the status travels as text because it is wrapped by async-tiff's
/// error type on the way out. `HttpReader` writes the marker; this reads it.
/// A typed error would need the upstream crate to carry one.
fn upstream_status(detail: &str) -> Option<u16> {
    let at = detail.find(super::cog::HTTP_STATUS_MARKER)?;
    detail[at + super::cog::HTTP_STATUS_MARKER.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

impl From<worker::Error> for Fail {
    fn from(e: worker::Error) -> Self {
        Self {
            status: 500,
            detail: e.to_string(),
        }
    }
}

pub(crate) type Out<T> = std::result::Result<T, Fail>;
