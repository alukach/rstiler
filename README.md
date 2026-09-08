# cf-geotiff-tileserver

A prototype of [titiler](https://github.com/developmentseed/titiler), written in
Rust and running on Cloudflare Workers. COGs are read over HTTP range requests
with [`async-tiff`](https://github.com/developmentseed/async-tiff), reprojected
to Web Mercator with [`proj4rs`](https://github.com/3liz/proj4rs), and encoded
to PNG — all inside the Worker, no GDAL, no Python.

## Endpoints

| Route | Notes |
|---|---|
| `GET /cog/info?url=` | size, bands, dtype, compression, nodata, overview count, bounds |
| `GET /cog/tilejson.json?url=` | TileJSON 2.2.0; min/max zoom derived from the overview chain |
| `GET /cog/tiles/{z}/{x}/{y}.png?url=` | the tile itself |
| `GET /cog/statistics?url=` | per-band min/max/mean/percentiles from a sample |
| `GET /cog/point/{lon},{lat}?url=` | raw band values under a WGS84 coordinate |
| `GET /cog/viewer` | example viewer (MapLibre + the source.coop example list) |
| `GET /` | 302 to `/cog/viewer`, query preserved |

Tile parameters use titiler's names and semantics: `bidx`, `rescale`,
`colormap_name`, `colormap`, `resampling` and `nodata`. See the parity table
below for exactly how far each goes.

The viewer takes `?url=`, `?bidx=` and `?rescale=` so a view is shareable, and otherwise offers the example list. `/cog/info` reports 2nd/98th
percentiles per band, which the viewer uses as the default rescale — without it
anything that isn't 8-bit renders black.

```
npm i -g wrangler          # needs node >= 22
wrangler dev
curl 'http://localhost:8787/cog/info?url=https://example.com/some.tif'
```

## How a tile is served

1. `TiffMetadataReader` walks the IFD chain over range requests. Mask IFDs
   (NewSubfileType bit 2) are dropped; the rest are the overview pyramid.
2. The COG's CRS comes from the GeoKeyDirectory, and `proj4rs` transforms the
   tile's Web Mercator bounds into it. A 9×9 probe grid — not just the corners,
   whose edges bow under reprojection — sizes the read window and gives the
   resolution the output needs, in the source CRS's own units.
3. That resolution selects the coarsest overview still finer than the output
   (`tiling::pick_overview`); ModelPixelScale + ModelTiepoint map the window to
   a set of TIFF tiles, fetched together and decoded.
4. Nearest-neighbour resample into 256×256, rescale to 8-bit, PNG out.

Every tile response carries a `Server-Timing` header splitting the work into
`meta` / `fetch` / `render`.

## Parity with titiler

These two tables are the project's map of what is and is not done. **They move
with the code** — see [AGENTS.md](AGENTS.md). The full write-up, with the
architecture comparison and the measurements behind it, lives at
<https://claude.ai/code/artifact/c51b79f7-e768-4024-b2e2-68c08b72ddfe>.

### Endpoints

| Route | titiler | here | Note |
|---|---|---|---|
| `/cog/tiles/{z}/{x}/{y}` | yes | png only | titiler also serves jpeg, webp, tiff, npy, jp2 |
| `/cog/tilejson.json` | yes | yes | zoom range derived from the overview chain |
| `/cog/info` | yes | extended | adds per-band 2nd/98th percentiles |
| `/cog/viewer` | yes | yes | MapLibre, with the source.coop example list |
| `/cog/statistics` | yes | partial | sampled from one overview level, not a full pass |
| `/cog/point/{lon},{lat}` | yes | yes | WGS84 only; no `coord_crs` |
| `/cog/preview` | yes | no | |
| `/cog/bbox/…` | yes | no | |
| `/cog/feature` | yes | no | needs geometry masking |
| `/cog/WMTSCapabilities.xml` | yes | no | |
| `/cog/validate` | yes | no | |
| `/cog/info.geojson`, `/cog/stac` | yes | no | |

Absent entirely: `titiler.mosaic` (MosaicJSON), `titiler.xarray` (Zarr/NetCDF),
the STAC surface, and the OpenAPI schema FastAPI generates for free.

### Tile parameters

| Parameter | here | Behaviour |
|---|---|---|
| `url` | yes | identical to titiler |
| `bidx` | yes | 1-based, comma separated; 1 or 3 bands |
| `rescale` | yes | `min,max`, or one pair per band separated by `;` |
| `nodata` | yes | overrides the dataset's `GDAL_NODATA` |
| `resampling` | partial | `nearest` (default) and `bilinear`; no cubic/lanczos/average |
| `colormap_name` | partial | `viridis`, `magma`, `plasma`, `inferno`, `cividis`, `terrain`, `greys` |
| `colormap` | partial | discrete `{"value":[r,g,b,a]}` for categorical data; no interval form |
| `expression` | no | no band math |
| `color_formula` | no | |
| `unscale` | no | internal scale/offset ignored |
| `buffer`, `padding` | no | |
| `algorithm` | no | titiler's plug-in post-processing hook |
| `tileMatrixSetId` | no | WebMercatorQuad is assumed, not selected |
| `tilesize` / `scale` | no | 256×256 fixed |

### Coordinate systems

EPSG-coded CRSs resolve from proj4rs's built-in table. A CRS written as
GeoKey 32767 — the projection spelled out in individual geokeys rather than
named, which is how USFS LCMS, NLCD and Vizzuality's Human Footprint all ship
— is rebuilt into a proj string covering Transverse Mercator, Mercator,
Lambert Conformal Conic (1SP and 2SP), Lambert Azimuthal Equal Area, Albers,
Azimuthal Equidistant, Equidistant Conic, Stereographic, Polar Stereographic,
Equirectangular and Sinusoidal, plus UTM named through `ProjectionGeoKey`.

Still unsupported: a rotated or sheared raster, which carries a
`ModelTransformation` instead of `ModelPixelScale` + `ModelTiepoint`.

### What has been verified

- A tile from a Web Mercator COG is **byte-identical** to `gdalwarp -r near` over
  the same bounds — max per-channel difference 0 across all 65,536 pixels.
- `proj4rs` agrees with `gdaltransform` to sub-millimetre on EPSG:32618, 2193 and
  4326. NAD83 codes such as 26918 differ by ~2 cm, since there are no datum grid
  shifts — three orders of magnitude below one pixel.
- `/cog/point` agrees with `gdallocationinfo -wgs84` exactly, and returns the
  same values for the same ground location across the 3857, 4326 and 32618
  builds of one scene — an independent check on the reprojection.
- A reconstructed Albers proj string agrees with `gdaltransform` to
  sub-millimetre, and `/cog/info` bounds match `gdalinfo` to eight decimals.
- **Five of the eight** [source.coop reference COGs](https://github.com/source-cooperative/cog-viewer)
  render, across EPSG:4326, 32618, 2193 and 26918. The three that don't: one is
  `PlanarConfiguration=2`, one is a user-defined Albers (GeoKey 32767), and one is
  3.7 GB with ~24.6k tiles per IFD and times out on the header alone.

### Where the time goes

Every response carries `Server-Timing`, reporting range reads and cache hits;
tiles additionally break out meta / fetch / render.

**Origins dominate, and they charge per request, not per byte.**
`data.source.coop` answers a range request in ~1.9–2.7 s whether you ask for
512 bytes or 1 MiB — connect is 8 ms, the rest is backend think-time. So
latency is essentially `requests × origin RTT`, and the only real lever is
making fewer requests.

Two changes follow from that, both shipped:

| | before | after |
|---|---|---|
| `/cog/info`, cold | 3.7 s | 3.7 s — one range read, which is the floor |
| `/cog/info`, warm | 3.7 s | **7 ms** (edge byte-range cache) |
| `/cog/info` computing percentiles | always | moved to `/cog/statistics` |

A 1 MiB initial readahead collapses the whole IFD walk into a *single* range
read on most COGs, and `HttpReader` caches each `(url, range)` in the Workers
Cache API, so every later request for the same COG header is edge-local.

Rendered tiles are cached too, keyed by crate version so a release that changes
rendering does not serve pixels drawn by the previous one.

But caching only helps the second visitor. The first one was paying for
metadata nobody asked for: `async-tiff`'s `read_all_ifds` materialises every
tag of every level, and `TileOffsets`/`TileByteCounts` hold one entry per tile.
A 463832 × 102252 raster carries 181,200 tile offsets at full resolution alone
— ~3.7 MiB of arrays across the chain — and `/cog/info` needs *none* of them,
while a tile needs one level's worth.

`src/lazyifd.rs` walks the chain reading only the small tags, then pulls the
per-tile arrays for the single level it will read pixels from. Reducing the
readahead does not help here and was measured: even at 16 KiB the old path
still pulled 3888 KiB, because the parse genuinely spans every array.

| cold, nothing cached | before | after |
|---|---|---|
| `/cog/info` | 3072 KiB, 0.49 s | **256 KiB, 0.24 s** |
| tile at z7 | 3785 KiB, 1.11 s | **1993 KiB, 0.65 s** |
| metadata parse (`meta;dur`) | 803 ms cold, 110 ms warm | **1–4 ms** |

Reading a deep array needs a different access pattern from walking the chain:
`ReadaheadMetadataCache` coalesces by reading sequentially *from the start of
the file*, which is right for the front matter and wrong for an array megabytes
in. `BlockFetch` coalesces around whatever it was asked for instead.

Rendering itself was never the problem — reprojection, resampling and PNG
encoding total 17–77 ms per tile.

### Next, in order of value

1. **ETag revalidation on the caches.** Both are time-based today, so a COG
   overwritten in place under the same URL serves stale bytes for a day.
2. **`/cog/preview` and `/cog/bbox`** — both fall out of generalising the tile
   pipeline to render an arbitrary window at an arbitrary size, which is worth
   doing on its own.
3. **`expression` band math** — the largest genuinely useful gap, and the one
   that needs a real parser.
4. **JPEG and WebP output** — `image-webp` already ships for decoding; a pure-Rust
   JPEG encoder would cut tile bytes substantially for imagery.
5. **Multiple TileMatrixSets** — deep rather than hard; the Web Mercator
   assumption is spread across bounds, overview selection and zoom derivation.

Also unsupported, with no plans: stripped (non-tiled) TIFFs, planar
configuration 2, JPEG2000/LERC/LZMA codecs, non-256 tile sizes.

## Notes on the dependencies

`async-tiff` is pinned to a git rev, not 0.3.0: the `async_trait(?Send)` cfgs
that make the reader traits usable on `wasm32-unknown-unknown` landed after that
release. With `default-features = false` (dropping `object_store` and `reqwest`)
it builds clean for wasm.

`HttpReader` implements `AsyncFileReader` on top of the Workers `fetch`
runtime, which is nearly all the porting `async-tiff` needed. It also overrides
`get_byte_ranges`: the trait's default awaits each range in turn, so a tile
needing N source tiles cost N round trips.

`async-tiff`'s WebP support binds libwebp, which is C and wants a wasm libc that
`wasm32-unknown-unknown` does not provide. `async-tiff` lets callers register
their own decoders, so `src/cog.rs` swaps in the pure-Rust `image-webp` instead.

`proj4rs` needs `default-features = false` (its default pulls in a `lazy_static`
multi-thread path) plus the `crs-definitions` feature for the EPSG table.
Spot-checked against `gdaltransform`, it agrees to sub-millimetre on 32618,
2193 and 4326; NAD83 codes such as 26918 differ by ~2 cm because there are no
datum grid shifts, which is far below one pixel.

## Layout

| File | Job |
|---|---|
| `src/lib.rs` | constants and the router |
| `src/cog.rs` | byte access, decoders, opening the IFD chain |
| `src/geo.rs` | CRS lookup, reprojection, affine transform, bounds |
| `src/lazyifd.rs` | walking the IFD chain without its per-tile arrays |
| `src/tiling.rs` | Web Mercator ↔ pixel math (pure, no deps) |
| `src/tiles.rs` | the tile handler: the resampling pipeline |
| `src/meta.rs` | `/cog/info` and `/cog/tilejson.json` |
| `src/render.rs` | query parameters, resampling, samples → 8-bit RGBA → PNG |
| `src/colormap.rs` | the colour tables (dependency-free, so it self-tests) |
| `src/viewer.html` | the example viewer |

## Local fixtures

One script, one flat directory, names that say what each file tests.

```
./fixtures/setup.sh              # synthetic only — no network, needs GDAL
./fixtures/setup.sh --all        # also pulls the shared corpus (~7 MB)
python3 fixtures/serve.py &      # :8099, with the Range support http.server lacks
wrangler dev &                   # :8787
python3 fixtures/check.py
```

`synthetic_*.tif` are generated locally from a ramp — no download, so they work
on a bad connection. One scene is built three ways (`_3857`, `_4326`, `_32618`);
because it is the same ground in each, `/cog/point` must return identical values
through all three, which is a direct check on the reprojection.

The rest come from
[developmentseed/geotiff-test-data](https://github.com/developmentseed/geotiff-test-data)
— the corpus `async-tiff` itself validates against — copied out under flat names
grouped by what they exercise: `compress_*`, `bands_*`, `layout_*`, `crs_*`.

## Tests

`fixtures/check.py` runs every fixture through `/cog/info`, `/cog/tilejson.json`
and a tile, then asserts the three things most expensive to get wrong: the
`gdalwarp` byte-identity, `/cog/point` agreeing across the three projections,
and colormaps producing colour.

Its `KNOWN_GAPS` dict is **the gap list in executable form** — each entry names a
fixture and why it cannot render. A file that fails without an entry is a
regression and fails the run; a file that starts passing prints `FIXED` and tells
you to delete its entry. Current state: **20 of 26 render, 6 declared gaps.**

| Gap | Fixture |
|---|---|
| `ModelTransformation` instead of `ModelPixelScale` | `crs_rotated_sar`, `layout_bigtiff_subifd` |
| `PlanarConfiguration=2` band reassembly | `layout_planar` |
| Stripped (non-tiled) TIFFs | `layout_stripped` |
| LERC — the decoder is C, needs a wasm libc | `compress_lerc` |
| JPEG-XL — `async-tiff` has no decoder | `compress_jpegxl` |

A blank tile fails too, unless the fixture is listed in `ALL_NODATA` —
`real_nlcd_landcover` is genuinely empty, which GDAL confirms with
`VALID_PERCENT=0`.

The pure-logic modules self-test without a wasm toolchain:

```
rustc --test src/tiling.rs   -o /tmp/t && /tmp/t
rustc --test src/colormap.rs -o /tmp/c && /tmp/c
```
