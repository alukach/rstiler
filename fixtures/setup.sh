#!/usr/bin/env bash
# Puts every test COG in this directory under a name that says what it tests.
#
#   ./fixtures/setup.sh          synthetic only — no network, needs GDAL
#   ./fixtures/setup.sh --all    also pulls the shared corpus (~7 MB)
#
set -euo pipefail
cd "$(dirname "$0")"

# ---------------------------------------------------------------- synthetic
# One scene, built from a ramp, reprojected three ways. Because it is the same
# ground in each, /cog/point must return identical values across all of them.
python3 - <<'PY'
w = h = 1024
rows = []
for y in range(h):
    r = bytes((x * 255) // w for x in range(w))
    g = bytes([(y * 255) // h]) * w
    b = bytes((((x // 64) + (y // 64)) % 2) * 200 + 30 for x in range(w))
    rows.append(bytes(v for t in zip(r, g, b) for v in t))
open("_src.ppm", "wb").write(b"P6\n%d %d\n255\n" % (w, h) + b"".join(rows))
PY

gdal_translate -q -a_srs EPSG:3857 -a_ullr -8250000 5000000 -8210000 4960000 _src.ppm _plain.tif
cog() { gdal_translate -q -of COG -co BLOCKSIZE=256 -co OVERVIEW_RESAMPLING=AVERAGE "$@"; }

cog -co COMPRESS=DEFLATE _plain.tif             synthetic_rgb_3857.tif
gdalwarp -q -overwrite -t_srs EPSG:4326  _plain.tif _a.tif && cog -co COMPRESS=DEFLATE _a.tif synthetic_rgb_4326.tif
gdalwarp -q -overwrite -t_srs EPSG:32618 _plain.tif _b.tif && cog -co COMPRESS=DEFLATE _b.tif synthetic_rgb_32618.tif
gdal_translate -q -b 1 -ot Int16 -scale 0 255 -2000 8000 -a_nodata -32768 _plain.tif _c.tif
cog -co COMPRESS=DEFLATE _c.tif                 synthetic_dem_int16.tif
gdal_translate -q -co TILED=YES -co BLOCKXSIZE=256 -co BLOCKYSIZE=256 -co COMPRESS=DEFLATE \
  _plain.tif                                    synthetic_no_overviews.tif

# Overviews appended *after* the image data, so the overview IFDs sit at the
# end of the file rather than the front. Legal TIFF, and how NLCD's CONUS land
# cover ships — a reader that walks the chain sequentially from byte 0 pulls
# the entire file to reach them.
# Uncompressed so the image data is bulky and the trailing IFDs land several
# blocks in, which is what makes a sequential reader visibly worse than a
# block one.
gdal_translate -q -co TILED=YES -co BLOCKXSIZE=256 -co BLOCKYSIZE=256 -co COMPRESS=NONE \
  _plain.tif                                    layout_trailing_overviews.tif
gdaladdo -q -r average layout_trailing_overviews.tif 2 4 8
rm -f _src.ppm _plain.tif _a.tif _b.tif _c.tif

# ---------------------------------------------------------------- corpus
if [ "${1:-}" = "--all" ]; then
  [ -d .corpus ] || git clone --depth 1 -q \
    https://github.com/developmentseed/geotiff-test-data .corpus
  R=.corpus/rasterio_generated/fixtures
  D=.corpus/real_data
  T=.corpus/tifffile_generated/fixtures

  # One file per capability, renamed to say which capability.
  cp "$R/uint8_rgb_webp_block64_cog.tif"                  compress_webp_rgb.tif
  cp "$R/uint8_rgba_webp_block64_cog.tif"                 compress_webp_rgba.tif
  cp "$R/uint8_1band_zstd_level1_block64.tif"             compress_zstd.tif
  cp "$R/uint8_1band_lzma_block64.tif"                    compress_lzma.tif
  cp "$R/uint8_1band_jxl_block64.tif"                     compress_jpegxl.tif
  cp "$R/float32_1band_lerc_block32.tif"                  compress_lerc.tif
  cp "$R/uint16_1band_lzw_block128_predictor2.tif"        compress_lzw_predictor.tif
  cp "$D/vantor/maxar_opendata_yellowstone_visual.tif"    compress_jpeg_rgb.tif

  cp "$R/cog_uint8_rgba.tif"                              bands_rgba.tif
  cp "$R/cog_uint8_rgb_mask.tif"                          bands_rgb_mask.tif
  cp "$R/cog_uint8_rgb_nodata.tif"                        bands_rgb_nodata.tif
  cp "$R/uint16_1band_scale_offset.tif"                   bands_scale_offset.tif
  cp "$R/int8_3band_zstd_block64.tif"                     layout_planar.tif
  cp "$R/uint8_1band_deflate_block128_unaligned.tif"      layout_unaligned_tiles.tif
  cp "$D/rio-tiler/non-tiled.tif"                         layout_stripped.tif
  cp "$T/subifd_pyramid_bigtiff.tif"                      layout_bigtiff_subifd.tif

  cp "$R/custom_crs.tif"                                  crs_user_defined.tif
  cp "$R/antimeridian.tif"                                crs_antimeridian.tif
  cp "$R/pixel_as_point.tif"                              crs_pixel_as_point.tif
  cp "$D/umbra/sydney_airport_GEC.tif"                    crs_rotated_sar.tif
  cp "$D/nlcd/nlcd_landcover.tif"                         real_nlcd_landcover.tif
fi

for f in ./*.tif; do
  printf '  %-30s %s\n' "${f#./}" "$(du -h "$f" | cut -f1)"
done
printf '  %s files\n' "$(find . -maxdepth 1 -name '*.tif' | wc -l | tr -d ' ')"
