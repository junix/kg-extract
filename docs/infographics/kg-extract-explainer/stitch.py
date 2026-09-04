#!/usr/bin/env python3
"""Stitch CDP slices into the 1:1 long-page bitmap + derive the bitmap set.

Hard-fails (exit != 0) on any of:
  - a missing or empty slice;
  - a slice whose bitmap size != manifest css size * dpr;
  - a stitched height != page CSS height * dpr (the empty/short-snapshot rule);
  - a blank page (every pixel one color).

Writes into the render dir: full@2x.png, full-gray.png, thumb.png (480 wide),
plus crops/crop-<n>.png per panel for the visual inspection record.
"""
import json
import os
import sys

from PIL import Image

render_dir = sys.argv[1] if len(sys.argv) > 1 else "render"
man = json.load(open(os.path.join(render_dir, "manifest.json")))
dpr = man["dpr"]

slices = []
for b in man["bitmaps"]:
    p = os.path.join(render_dir, b["name"])
    if not os.path.exists(p):
        sys.exit(f"FATAL: missing slice {b['name']}")
    if os.path.getsize(p) < 1024:
        sys.exit(f"FATAL: slice {b['name']} looks empty ({os.path.getsize(p)} bytes)")
    img = Image.open(p)
    ew, eh = round(b["cssH"] * 0 + man["cssWidth"] * dpr), round(b["cssH"] * dpr)
    if img.size != (ew, eh):
        sys.exit(f"FATAL: slice {b['name']} is {img.size}, expected {(ew, eh)}")
    slices.append(img)

W = man["cssWidth"] * dpr
H = man["cssHeight"] * dpr
if sum(s.size[1] for s in slices) != H:
    sys.exit(f"FATAL: stitched height {sum(s.size[1] for s in slices)} != css {H}")

full = Image.new("RGB", (W, H))
y = 0
for s in slices:
    full.paste(s, (0, y))
    y += s.size[1]

# blank-page guard: an all-one-color render means nothing drew
colors = full.convert("L").getcolors(maxcolors=2)
if colors is not None:
    sys.exit(f"FATAL: page rendered blank (single gray level): {colors}")

full.save(os.path.join(render_dir, "full@2x.png"))
full.convert("L").save(os.path.join(render_dir, "full-gray.png"))
full.resize((480, round(H * 480 / W)), Image.LANCZOS).save(
    os.path.join(render_dir, "thumb.png"))

crops_dir = os.path.join(render_dir, "crops")
os.makedirs(crops_dir, exist_ok=True)
records = []
for i, p in enumerate(man.get("panels", [])):
    x0 = round(p["x"] * dpr)
    y0 = round(p["y"] * dpr)
    x1 = round((p["x"] + p["w"]) * dpr)
    y1 = round((p["y"] + p["h"]) * dpr)
    crop = full.crop((max(0, x0), max(0, y0), min(W, x1), min(H, y1)))
    name = f"crop-{i:02d}-{os.path.basename(p['src'])}.png"
    crop.save(os.path.join(crops_dir, name))
    records.append({"panel": p["src"], "css_y": p["y"], "crop": f"crops/{name}"})
json.dump(records, open(os.path.join(render_dir, "crops.json"), "w"),
          ensure_ascii=False, indent=1)

print(f"stitched {W}x{H} (css {man['cssWidth']}x{man['cssHeight']} @ dpr {dpr})")
print(f"bitmaps: full@2x.png / full-gray.png / thumb.png + {len(records)} crops")
