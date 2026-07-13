#!/usr/bin/env python3
"""Generate tray / menubar / AppIcon from SVG + brand PNG masters.

Masters (edit these):
  assets/syncing1.svg … syncing6.svg   blue cloud + white arrows (animation)
  assets/syncing.svg                   = syncing1 (canonical)
  assets/complete.svg                  green cloud + check
  assets/brand.png                     shield (Frame-1) → app-idle / AppIcon / menubar idle

Derived:
  syncing.ico, syncing2..7.ico, complete.ico
  menubar-icon.png, menubar-syncing.png, menubar-complete.png
  app-idle.ico, AppIcon.icns
  bridge-*.png from bridge-*.svg

Requires: cairosvg, Pillow, magick, iconutil
"""
from __future__ import annotations

import shutil
import subprocess
import tempfile
from io import BytesIO
from pathlib import Path

import cairosvg
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"


def svg_png(svg: Path, size: int) -> Image.Image:
    data = cairosvg.svg2png(url=str(svg), output_width=size, output_height=size)
    return Image.open(BytesIO(data)).convert("RGBA")


def magick_ico(png: Path, ico: Path, sizes: str) -> None:
    subprocess.check_call(
        ["magick", str(png), "-define", f"icon:auto-resize={sizes}", str(ico)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def to_menubar_template(im: Image.Image, *, white_is_hole: bool) -> Image.Image:
    """Black template for macOS.

    white_is_hole=True  → syncing/complete (white arrows stay cutouts)
    white_is_hole=False → brand (white fill stays solid in silhouette)
    """
    out = Image.new("RGBA", im.size, (0, 0, 0, 0))
    sp, dp = im.load(), out.load()
    for y in range(im.size[1]):
        for x in range(im.size[0]):
            r, g, b, a = sp[x, y]
            if a < 12:
                continue
            if white_is_hole and r > 240 and g > 240 and b > 240:
                continue
            dp[x, y] = (0, 0, 0, a)
    return out


def build_appicon(brand: Image.Image) -> None:
    iconset = Path(tempfile.mkdtemp(suffix=".iconset"))
    try:
        mapping = {
            "icon_16x16.png": 16,
            "diana.k@example.org": 32,
            "icon_32x32.png": 32,
            "ivan.p@example.net": 64,
            "icon_128x128.png": 128,
            "wendy.h@example.net": 256,
            "icon_256x256.png": 256,
            "wendy.h@example.net": 512,
            "icon_512x512.png": 512,
            "walt.e@example.net": 1024,
        }
        for name, size in mapping.items():
            brand.resize((size, size), Image.Resampling.LANCZOS).save(
                iconset / name, format="PNG"
            )
        subprocess.check_call(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(ASSETS / "AppIcon.icns")],
            stdout=subprocess.DEVNULL,
        )
    finally:
        shutil.rmtree(iconset, ignore_errors=True)


def main() -> None:
    tmp = Path(tempfile.mkdtemp())
    try:
        # --- Syncing frames from SVG ---
        frames = []
        for i in range(1, 7):
            svg = ASSETS / f"syncing{i}.svg"
            if not svg.is_file():
                raise SystemExit(f"missing {svg}")
            im = svg_png(svg, 256)
            frames.append(im)
            png = tmp / f"sync{i}.png"
            im.save(png)
            if i == 1:
                magick_ico(png, ASSETS / "syncing.ico", "256,128,64,48,32,16")
            # syncing2..7 ← syncing1..6
            magick_ico(png, ASSETS / f"syncing{i + 1}.ico", "64,48,32,16")
        print("wrote syncing.ico + syncing2..7.ico from syncing1..6.svg")

        # --- Complete from SVG ---
        done = svg_png(ASSETS / "complete.svg", 256)
        done_png = tmp / "done.png"
        done.save(done_png)
        magick_ico(done_png, ASSETS / "complete.ico", "256,128,64,48,32,16")
        print("wrote complete.ico from complete.svg")

        # --- Brand from Frame-1 PNG ---
        brand_path = ASSETS / "brand.png"
        if not brand_path.is_file():
            raise SystemExit("missing assets/brand.png (Frame-1)")
        brand = Image.open(brand_path).convert("RGBA")
        brand_png = tmp / "brand.png"
        brand.save(brand_png)
        magick_ico(brand_png, ASSETS / "app-idle.ico", "256,128,64,48,32,16")
        build_appicon(brand)
        print("wrote app-idle.ico + AppIcon.icns from brand.png")

        # --- Menubar templates @22 ---
        to_menubar_template(brand, white_is_hole=False).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-icon.png")
        to_menubar_template(frames[0], white_is_hole=True).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-syncing.png")
        to_menubar_template(done, white_is_hole=True).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-complete.png")
        print("wrote menubar-*.png")

        # --- Bridge from SVG ---
        for name, size in [("bridge-pc", 40), ("bridge-server", 32)]:
            svg = ASSETS / f"{name}.svg"
            if svg.is_file():
                svg_png(svg, size).save(ASSETS / f"{name}.png")
        # Prefer brand-plate (Frame.png) for bridge-server if present — white glyph on green
        plate = ASSETS / "brand-plate.png"
        if plate.is_file():
            Image.open(plate).convert("RGBA").resize(
                (32, 32), Image.Resampling.LANCZOS
            ).save(ASSETS / "bridge-server.png")
        print("wrote bridge-*.png")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("done — icons from SVG/brand masters")


if __name__ == "__main__":
    main()
