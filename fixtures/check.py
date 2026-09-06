#!/usr/bin/env python3
"""Run every fixture through the tiler and gate on regressions.

    python3 fixtures/serve.py &     # :8099
    wrangler dev &                  # :8787
    python3 fixtures/check.py

Every `.tif` here gets an /cog/info and a tile. Files listed in KNOWN_GAPS are
allowed to fail, with the reason recorded — that list is the gap list, in
executable form. A file that fails without being listed is a regression; a file
that starts passing tells you to delete its entry.
"""
import json, math, os, pathlib, struct, subprocess, sys, urllib.parse, zlib

TILER = os.environ.get("TILER", "http://127.0.0.1:8787")
ORIGIN = os.environ.get("ORIGIN", "http://127.0.0.1:8099")
HERE = pathlib.Path(__file__).parent

# Fixture -> why it cannot render yet. Delete an entry when you fix it.
KNOWN_GAPS = {
    "compress_lerc.tif":        "LERC decoder is C (lerc-sys); needs a wasm libc",
    "compress_jpegxl.tif":      "async-tiff has no JPEG-XL decoder",
    "layout_planar.tif":        "PlanarConfiguration=2 band reassembly",
    "layout_stripped.tif":      "stripped (non-tiled) TIFFs",
    "layout_bigtiff_subifd.tif": "SubIFD pyramid carries no ModelPixelScale",
    "crs_user_defined.tif":     "GeoKey 32767; proj string must be rebuilt from geokeys",
    "real_nlcd_landcover.tif":  "GeoKey 32767 (Albers)",
    "crs_rotated_sar.tif":      "ModelTransformation instead of ModelPixelScale",
}


def curl(path, out="/tmp/check_body.bin", timeout=120):
    code = subprocess.run(
        ["curl", "-s", "--max-time", str(timeout), "-o", out, "-w", "%{http_code}",
         TILER + path], capture_output=True, text=True).stdout.strip()
    return code, pathlib.Path(out)


def detail(p):
    try:
        return json.loads(p.read_text(errors="replace")).get("detail", "?")
    except Exception:
        return "no response"


def load_png(path):
    raw = pathlib.Path(path).read_bytes()
    i, w, h, ct, idat = 8, 0, 0, 0, b""
    while i < len(raw):
        ln = struct.unpack(">I", raw[i:i + 4])[0]
        tag, data = raw[i + 4:i + 8], raw[i + 8:i + 8 + ln]
        if tag == b"IHDR":
            w, h, _, ct = struct.unpack(">IIBB", data[:10])
        elif tag == b"IDAT":
            idat += data
        i += 12 + ln
    n = {0: 1, 2: 3, 6: 4}[ct]
    dat, out, prev, p = zlib.decompress(idat), [], bytearray(w * n), 0
    for _ in range(h):
        f = dat[p]
        line = bytearray(dat[p + 1:p + 1 + w * n])
        p += 1 + w * n
        for k in range(len(line)):
            a = line[k - n] if k >= n else 0
            b = prev[k]
            c = prev[k - n] if k >= n else 0
            if f == 1: line[k] = (line[k] + a) & 255
            elif f == 2: line[k] = (line[k] + b) & 255
            elif f == 3: line[k] = (line[k] + (a + b) // 2) & 255
            elif f == 4:
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                line[k] = (line[k] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out.append(bytes(line))
        prev = line
    return w, h, n, b"".join(out)


def opaque(path):
    w, h, n, d = load_png(path)
    return sum(1 for i in range(w * h) if n < 4 or d[i * n + 3] > 0)


def maxdiff(a, b):
    w, h, n, da = load_png(a)
    _, _, n2, db = load_png(b)
    return max(abs(da[i * n + c] - db[i * n2 + c]) for i in range(w * h) for c in range(3))


def url_for(name):
    return urllib.parse.quote(f"{ORIGIN}/{name}", safe="")


def center_tile(bounds, z):
    lon, lat = (bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2
    lat = max(min(lat, 85.05), -85.05)
    n = 2 ** z
    return z, min(int((lon + 180) / 360 * n), n - 1), \
        min(max(int((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n), 0), n - 1)


def sweep():
    """Returns (regressions, newly_fixed)."""
    regressions, fixed = [], []
    for f in sorted(HERE.glob("*.tif")):
        name = f.name
        q = url_for(name)
        why = None

        code, body = curl(f"/cog/info?url={q}")
        if code != "200":
            why = detail(body)
        else:
            code, body = curl(f"/cog/tilejson.json?url={q}")
            if code != "200":
                why = detail(body)
            else:
                tj = json.loads(body.read_text())
                z, x, y = center_tile(tj["bounds"], max(tj["minzoom"], min(tj["maxzoom"], tj["minzoom"] + 2)))
                code, body = curl(f"/cog/tiles/{z}/{x}/{y}.png?url={q}", "/tmp/check_tile.png")
                if code != "200":
                    why = detail(body)

        expected = KNOWN_GAPS.get(name)
        if why and expected:
            print(f"  gap   {name:<30} {expected}")
        elif why:
            print(f"  FAIL  {name:<30} {why[:70]}")
            regressions.append(name)
        elif expected:
            print(f"  FIXED {name:<30} remove it from KNOWN_GAPS")
            fixed.append(name)
        else:
            print(f"  ok    {name:<30} {opaque('/tmp/check_tile.png')} opaque px")
    return regressions, fixed


def assertions():
    """The claims that are easiest to break and most expensive to get wrong."""
    bad = []
    Z, X, Y = 12, 1204, 1539

    # 1. No reprojection needed, so it must match GDAL exactly.
    curl(f"/cog/tiles/{Z}/{X}/{Y}.png?url={url_for('synthetic_rgb_3857.tif')}", "/tmp/m.png")
    R = 20037508.342789244
    s = 2 * R / (1 << Z)
    te = (-R + X * s, R - (Y + 1) * s, -R + (X + 1) * s, R - Y * s)
    if subprocess.run(["gdalwarp", "-q", "-overwrite", "-t_srs", "EPSG:3857", "-te",
                       *map(str, te), "-ts", "256", "256", "-r", "near",
                       str(HERE / "synthetic_rgb_3857.tif"), "/tmp/ref.tif"]).returncode == 0:
        subprocess.run(["gdal_translate", "-q", "-of", "PNG", "-b", "1", "-b", "2", "-b", "3",
                        "/tmp/ref.tif", "/tmp/ref.png"], check=True)
        d = maxdiff("/tmp/m.png", "/tmp/ref.png")
        print(f"  {'ok' if d == 0 else 'FAIL'}    gdalwarp byte-identity          max channel diff {d}")
        if d: bad.append("gdalwarp")

    # 2. The same ground point must read the same through three projections.
    want = subprocess.run(["gdallocationinfo", "-wgs84", "-valonly",
                           str(HERE / "synthetic_rgb_3857.tif"), "-73.93", "40.80"],
                          capture_output=True, text=True).stdout.split()
    for v in ("3857", "4326", "32618"):
        _, body = curl(f"/cog/point/-73.93,40.80?url={url_for(f'synthetic_rgb_{v}.tif')}")
        got = [str(int(x)) for x in json.loads(body.read_text())["values"]]
        okp = got == want
        print(f"  {'ok' if okp else 'FAIL'}    point through EPSG:{v:<6}        {','.join(got)}")
        if not okp: bad.append(f"point/{v}")

    # 3. A colormap must produce colour, not grey.
    base = f"/cog/tiles/{Z}/{X}/{Y}.png?url={url_for('synthetic_dem_int16.tif')}&rescale=-2000,8000"
    curl(base + "&colormap_name=viridis", "/tmp/v.png")
    curl(base + "&colormap_name=greys", "/tmp/g.png")
    d = maxdiff("/tmp/v.png", "/tmp/g.png")
    print(f"  {'ok' if d > 20 else 'FAIL'}    colormap viridis vs greys       max channel diff {d}")
    if d <= 20: bad.append("colormap")
    return bad


if __name__ == "__main__":
    if not list(HERE.glob("*.tif")):
        sys.exit("no fixtures — run ./fixtures/setup.sh first")
    regressions, fixed = sweep()
    print()
    bad = assertions()
    total = len(list(HERE.glob("*.tif")))
    print(f"\n  {total - len(KNOWN_GAPS) - len(regressions)}/{total} render, "
          f"{len(KNOWN_GAPS)} known gaps")
    if regressions or bad or fixed:
        print(f"\n  regressions: {regressions + bad}" if regressions or bad else "")
        sys.exit(1)
