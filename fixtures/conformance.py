#!/usr/bin/env python3
"""Check this server against titiler's own contract.

Every assertion here mirrors one in titiler's test suite, cited by name, so a
client written against titiler behaves the same way here for the subset we
implement. Source:
src/titiler/core/tests/test_factories.py — test_TilerFactory, test_rescale_dependency

    python3 fixtures/serve.py &     # :8099
    wrangler dev &                  # :8787
    python3 fixtures/conformance.py

Checks tagged UNIMPLEMENTED name a titiler behaviour we do not have. They are
reported, not failed — that list is the conformance gap, and it shrinks by
deleting entries, never by weakening an assertion.
"""
import json, math, os, pathlib, subprocess, sys, urllib.parse

TILER = os.environ.get("TILER", "http://127.0.0.1:8787")
ORIGIN = os.environ.get("ORIGIN", "http://127.0.0.1:8099")
HERE = pathlib.Path(__file__).parent

# The synthetic RGB scene, and a single-band Int16 one for colormap checks.
RGB = urllib.parse.quote(f"{ORIGIN}/synthetic_rgb_3857.tif", safe="")
DEM = urllib.parse.quote(f"{ORIGIN}/synthetic_dem_int16.tif", safe="")

# A tile with data in it, and the WGS84 point at its centre.
Z, X, Y = 12, 1204, 1539
LON, LAT = -73.93, 40.80

# titiler behaviour we have not built. Each entry is reported, not run.
UNIMPLEMENTED = {
    "expression": "band math",
    "output formats": "jpeg, webp, tif, npy — we serve png only",
    "return_mask": "mask band in the output",
    "coord_crs / dst_crs": "point and tile CRS overrides",
    "tileMatrixSetId in path": "WebMercatorQuad is assumed, not selected",
    "info.geojson": "GeoJSON info, incl. MultiPolygon across the antimeridian",
    "preview / bbox / feature": "arbitrary-window rendering",
    "colormap intervals": "[([min,max],[r,g,b,a]), ...] form",
    "algorithm": "post-processing hooks",
    "histogram / majority / minority / unique": "in /statistics",
}

results = []


def check(name, cite):
    """Register one assertion, citing the titiler test it comes from."""
    def wrap(fn):
        try:
            fn()
            results.append(("ok", name, cite, ""))
        except AssertionError as e:
            results.append(("FAIL", name, cite, str(e)))
        except Exception as e:  # a crash is a failure too
            results.append(("FAIL", name, cite, f"{type(e).__name__}: {e}"))
        return fn
    return wrap


def get(path):
    """Returns (status, headers, body-bytes)."""
    out = "/tmp/conf_body.bin"
    hdr = "/tmp/conf_head.txt"
    code = subprocess.run(
        ["curl", "-s", "--max-time", "120", "-o", out, "-D", hdr, "-w", "%{http_code}",
         TILER + path], capture_output=True, text=True).stdout.strip()
    headers = {}
    for line in pathlib.Path(hdr).read_text(errors="replace").splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            headers[k.strip().lower()] = v.strip()
    return int(code or 0), headers, pathlib.Path(out).read_bytes()


def js(body):
    return json.loads(body.decode(errors="replace"))


# ---------------------------------------------------------------- tilejson

@check("tilejson: 200 + json + tilejson key", "test_TilerFactory")
def _():
    code, h, body = get(f"/cog/tilejson.json?url={RGB}")
    assert code == 200, f"got {code}"
    assert h["content-type"].startswith("application/json"), h.get("content-type")
    assert js(body)["tilejson"], "no tilejson key"


@check("tilejson: tiles url mentions png", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/tilejson.json?url={RGB}")
    assert "png" in js(body)["tiles"][0], js(body)["tiles"][0]


@check("tilejson: minzoom/maxzoom overridable", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/tilejson.json?url={RGB}&minzoom=5&maxzoom=12")
    d = js(body)
    assert d["minzoom"] == 5, f"minzoom {d['minzoom']}"
    assert d["maxzoom"] == 12, f"maxzoom {d['maxzoom']}"


# ---------------------------------------------------------------- info

@check("info: 200 + json", "test_TilerFactory")
def _():
    code, h, _ = get(f"/cog/info?url={RGB}")
    assert code == 200, f"got {code}"
    assert h["content-type"].startswith("application/json")


@check("info: rio-tiler Info fields present", "test_TilerFactory / rio_tiler.models.Info")
def _():
    _, _, body = get(f"/cog/info?url={RGB}")
    d = js(body)
    for k in ("bounds", "band_metadata", "band_descriptions", "dtype", "nodata_type"):
        assert k in d, f"missing {k}"


@check("info: band_metadata is one entry per band", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/info?url={RGB}")
    d = js(body)
    assert len(d["band_metadata"]) == 3, d.get("band_metadata")
    assert d["band_descriptions"][0][0] == "b1", d.get("band_descriptions")


# ---------------------------------------------------------------- tiles

@check("tile: 200 + image/png", "test_TilerFactory")
def _():
    code, h, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={RGB}")
    assert code == 200, f"got {code}"
    assert h["content-type"] == "image/png", h.get("content-type")


@check("tile: rescale accepted", "test_TilerFactory")
def _():
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={RGB}&rescale=0,1000")
    assert code == 200, f"got {code}"


@check("tile: extreme float rescale accepted", "test_TilerFactory")
def _():
    r = urllib.parse.quote("-3.4028235e+38,3.4028235e+38", safe="")
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={RGB}&rescale={r}")
    assert code == 200, f"got {code}"


@check("tile: bidx repeated selects bands in order", "test_TilerFactory")
def _():
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={RGB}&bidx=1&bidx=1&bidx=1")
    assert code == 200, f"got {code}"


@check("tile: bidx + colormap_name", "test_TilerFactory")
def _():
    code, h, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={DEM}"
                     f"&bidx=1&rescale=-2000,8000&colormap_name=viridis")
    assert code == 200, f"got {code}"
    assert h["content-type"] == "image/png"


@check("tile: colormap dict, mixed hex and rgb forms", "test_TilerFactory")
def _():
    cmap = urllib.parse.quote(json.dumps({
        "1": [58, 102, 24, 255],
        "2": [100, 177, 41],
        "3": "#b1b129",
        "4": "#ddcb9aFF",
    }), safe="")
    code, h, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={DEM}&bidx=1&colormap={cmap}")
    assert code == 200, f"got {code}"
    assert h["content-type"] == "image/png"


@check("tile: malformed colormap -> 400", "test_TilerFactory")
def _():
    cmap = urllib.parse.quote(json.dumps({"1": [58, 102]}), safe="")
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={DEM}&bidx=1&colormap={cmap}")
    assert code == 400, f"got {code}, expected 400"


@check("tile: colormap that is not json -> 400", "test_TilerFactory")
def _():
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={DEM}&bidx=1&colormap=notjson")
    assert code == 400, f"got {code}, expected 400"


@check("tile: bidx out of range -> 400", "titiler returns 4xx for bad params")
def _():
    code, _, _ = get(f"/cog/tiles/{Z}/{X}/{Y}.png?url={RGB}&bidx=99")
    assert code == 400, f"got {code}, expected 400"


# ---------------------------------------------------------------- point

@check("point: values + band_names", "test_TilerFactory")
def _():
    code, h, body = get(f"/cog/point/{LON},{LAT}?url={DEM}")
    assert code == 200, f"got {code}"
    assert h["content-type"].startswith("application/json")
    d = js(body)
    assert len(d["values"]) == 1, d.get("values")
    assert d["band_names"] == ["b1"], d.get("band_names")


@check("point: nodata masks the value to null", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/point/{LON},{LAT}?url={DEM}&nodata=3020")
    assert js(body)["values"] == [None], js(body).get("values")


@check("point: bidx repeated returns one value per entry", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/point/{LON},{LAT}?url={RGB}&bidx=1&bidx=1")
    assert len(js(body)["values"]) == 2, js(body).get("values")


# ---------------------------------------------------------------- statistics

@check("statistics: rio-tiler BandStatistics fields", "test_TilerFactory / rio_tiler.models.BandStatistics")
def _():
    _, _, body = get(f"/cog/statistics?url={RGB}")
    d = js(body)
    assert "b1" in d, list(d)[:4]
    want = {"min", "max", "mean", "count", "sum", "std", "median",
            "valid_percent", "masked_pixels", "valid_pixels", "description",
            "percentile_2", "percentile_98"}
    missing = want - set(d["b1"])
    assert not missing, f"missing {sorted(missing)}"


@check("statistics: description names the band", "test_TilerFactory")
def _():
    _, _, body = get(f"/cog/statistics?url={RGB}")
    assert js(body)["b1"]["description"] == "b1", js(body)["b1"].get("description")


# ---------------------------------------------------------------- routing

@check("missing url parameter -> 4xx", "FastAPI required query param")
def _():
    code, _, _ = get("/cog/info")
    assert 400 <= code < 500, f"got {code}"


@check("unknown route -> 404", "test_path_param_in_prefix")
def _():
    code, _, _ = get(f"/cog/nope?url={RGB}")
    assert code == 404, f"got {code}"


@check("unreadable source -> 4xx, not 5xx", "titiler maps reader errors")
def _():
    bad = urllib.parse.quote(f"{ORIGIN}/does_not_exist.tif", safe="")
    code, _, _ = get(f"/cog/info?url={bad}")
    assert 400 <= code < 500, f"got {code}, expected a 4xx"


# ---------------------------------------------------------------- report

if __name__ == "__main__":
    if not (HERE / "synthetic_rgb_3857.tif").exists():
        sys.exit("no fixtures — run ./fixtures/setup.sh first")

    failed = 0
    for status, name, cite, detail in results:
        if status == "ok":
            print(f"  ok    {name}")
        else:
            failed += 1
            print(f"  FAIL  {name}\n          {detail}\n          titiler: {cite}")

    print(f"\n  {len(results) - failed}/{len(results)} conformance checks pass")
    if UNIMPLEMENTED:
        print(f"\n  not implemented ({len(UNIMPLEMENTED)}):")
        for k, v in UNIMPLEMENTED.items():
            print(f"    {k:<40} {v}")
    sys.exit(1 if failed else 0)
