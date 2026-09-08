//! Getting a COG open: byte access, decompression, and the IFD chain.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_tiff::decoder::{Decoder, DecoderRegistry};
use async_tiff::error::{AsyncTiffError, AsyncTiffResult};
use async_tiff::reader::AsyncFileReader;
use async_tiff::{ImageFileDirectory, TIFF};

use crate::fail::{Fail, Out};
use crate::lazyifd::{BlockFetch, Levels, CHAIN_BLOCK};
use async_trait::async_trait;
use bytes::Bytes;
use worker::*;

/// Reads byte ranges out of a remote COG using the Workers `fetch` runtime.
#[derive(Debug, Clone)]
pub(crate) struct HttpReader {
    pub(crate) url: String,
    /// Range requests issued so far, and how many the edge cache served.
    /// Origins commonly charge ~2 s of think-time per range request regardless
    /// of size, so these two numbers explain nearly all of a response's latency.
    reqs: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
    bytes: Arc<AtomicUsize>,
}

impl HttpReader {
    pub(crate) fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            reqs: Arc::new(AtomicUsize::new(0)),
            hits: Arc::new(AtomicUsize::new(0)),
            bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// `(range requests issued, of which served from cache, bytes read)`.
    pub(crate) fn traffic(&self) -> (usize, usize, usize) {
        (
            self.reqs.load(Ordering::Relaxed),
            self.hits.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }

    /// A synthetic key naming this exact byte range. Never fetched — the Cache
    /// API only uses it as an identifier, and it is unique per (url, range).
    fn cache_key(&self, range: &Range<u64>) -> String {
        let sep = if self.url.contains('?') { '&' } else { '?' };
        format!("{}{sep}__range={}-{}", self.url, range.start, range.end)
    }
}

/// Prefix that carries an origin status out through async-tiff's error type,
/// so the router can map it to a status of its own. See `fail::upstream`.
pub(crate) const HTTP_STATUS_MARKER: &str = "upstream HTTP ";

fn tiff_err<E: std::fmt::Debug>(e: E) -> AsyncTiffError {
    AsyncTiffError::General(format!("{e:?}"))
}

#[async_trait(?Send)]
impl AsyncFileReader for HttpReader {
    async fn get_bytes(&self, range: Range<u64>) -> AsyncTiffResult<Bytes> {
        self.reqs.fetch_add(1, Ordering::Relaxed);
        let key = self.cache_key(&range);
        let cache = Cache::default();

        // A COG reads the same header offsets on every request, so the edge
        // cache turns the second and later visits into local reads.
        if let Ok(Some(mut hit)) = cache.get(&key, true).await {
            if let Ok(body) = hit.bytes().await {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(body.len(), Ordering::Relaxed);
                return Ok(Bytes::from(body));
            }
        }

        let headers = Headers::new();
        headers
            .set("Range", &format!("bytes={}-{}", range.start, range.end - 1))
            .map_err(tiff_err)?;
        let req = Request::new_with_init(
            &self.url,
            RequestInit::new()
                .with_method(Method::Get)
                .with_headers(headers),
        )
        .map_err(tiff_err)?;

        let mut resp = Fetch::Request(req).send().await.map_err(tiff_err)?;
        if resp.status_code() != 206 && resp.status_code() != 200 {
            return Err(AsyncTiffError::General(format!(
                "{HTTP_STATUS_MARKER}{} for {}",
                resp.status_code(),
                self.url
            )));
        }
        let body = resp.bytes().await.map_err(tiff_err)?;
        self.bytes.fetch_add(body.len(), Ordering::Relaxed);

        // ponytail: time-based, with no ETag revalidation. A COG that is
        // overwritten in place under the same URL would serve stale bytes for a
        // day; key on the ETag if that ever becomes a real workflow.
        if let Ok(mut store) = Response::from_bytes(body.clone()) {
            let _ = store
                .headers_mut()
                .set("Cache-Control", "public, max-age=86400");
            let _ = cache.put(&key, store).await;
        }
        Ok(Bytes::from(body))
    }

    /// The trait's default implementation awaits each range in turn, so a tile
    /// needing N source tiles cost N round trips. Issue them together instead;
    /// the runtime caps concurrent connections and queues the rest.
    async fn get_byte_ranges(&self, ranges: Vec<Range<u64>>) -> AsyncTiffResult<Vec<Bytes>> {
        futures::future::try_join_all(ranges.into_iter().map(|r| self.get_bytes(r))).await
    }
}

/// async-tiff's built-in WebP decoder binds libwebp, which needs a C sysroot
/// wasm32-unknown-unknown does not have. `image-webp` is pure Rust, and
/// async-tiff lets us swap it in.
#[derive(Debug)]
struct WebPDecoder;

impl Decoder for WebPDecoder {
    fn decode_tile(
        &self,
        buffer: Bytes,
        _photometric_interpretation: async_tiff::tags::PhotometricInterpretation,
        _jpeg_tables: Option<&[u8]>,
        samples_per_pixel: u16,
        _bits_per_sample: u16,
        _lerc_parameters: Option<&[u32]>,
    ) -> AsyncTiffResult<Vec<u8>> {
        let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(buffer))
            .map_err(|e| AsyncTiffError::General(format!("webp: {e}")))?;
        let (w, h) = d.dimensions();
        let got = if d.has_alpha() { 4 } else { 3 };
        let mut out = vec![0u8; w as usize * h as usize * got];
        d.read_image(&mut out)
            .map_err(|e| AsyncTiffError::General(format!("webp: {e}")))?;

        // GDAL can declare 4 samples on an IFD whose WebP payload carries only
        // RGB; pad the missing samples opaque so the array shape still matches.
        let want = samples_per_pixel as usize;
        if want > got {
            let mut padded = vec![255u8; w as usize * h as usize * want];
            for (src, dst) in out.chunks(got).zip(padded.chunks_mut(want)) {
                dst[..got].copy_from_slice(src);
            }
            return Ok(padded);
        }
        Ok(out)
    }
}

pub(crate) fn decoders() -> DecoderRegistry {
    let mut r = DecoderRegistry::default();
    r.as_mut().insert(
        async_tiff::tags::Compression::WebP,
        Box::new(WebPDecoder) as _,
    );
    r
}

/// An open COG: the reader, the metadata cache in front of it, and the IFD
/// chain parsed without its per-tile arrays.
pub(crate) struct Cog {
    pub(crate) reader: HttpReader,
    levels: Levels,
}

impl Cog {
    pub(crate) async fn open(src: &str) -> Out<Self> {
        let reader = HttpReader::new(src);
        // Read the chain in blocks, not sequentially from byte 0. A COG is
        // *supposed* to keep its IFDs at the front, but plenty do not — NLCD's
        // CONUS land cover puts its overview IFDs 962 MB into a 1.4 GB file,
        // and a sequential readahead walking out to them pulls the whole
        // gigabyte through a Worker that is capped at 128 MB.
        let fetch = BlockFetch::with_block(reader.clone(), CHAIN_BLOCK);
        let levels = Levels::open(&fetch)
            .await
            .map_err(|e| Fail::upstream(format!("could not read metadata: {e}")))?;
        if levels.light().is_empty() {
            return Err(Fail::bad("no image IFDs in this TIFF"));
        }
        Ok(Self { reader, levels })
    }

    /// Per-level metadata, cheap — no per-tile arrays were read.
    pub(crate) fn levels(&self) -> &[ImageFileDirectory] {
        self.levels.light()
    }

    /// Read one level with its per-tile arrays, so its pixels can be fetched.
    pub(crate) async fn full(&self, level: usize) -> Out<TIFF> {
        self.levels
            .full(&self.reader, level)
            .await
            .map_err(|e| Fail::upstream(format!("could not read level {level}: {e}")))
    }

    /// Index of the coarsest level, which is where sampling for statistics
    /// costs least.
    pub(crate) fn coarsest(&self) -> usize {
        self.levels.light().len() - 1
    }
}

pub(crate) fn nodata_of(ifd: &ImageFileDirectory) -> Option<f64> {
    ifd.gdal_nodata().and_then(|s| s.trim().parse().ok())
}
