#!/usr/bin/env python3
"""Generate tray / menubar / AppIcon from shield SVG masters.

Masters (edit these only):
  assets/originals/idle.svg
  assets/originals/complete.svg
  assets/originals/syncing1.svg … syncing6.svg   (syncing.svg = syncing1)

Derived (generated, do not hand-edit): app-idle.ico, AppIcon.icns, syncing*.ico, complete.ico, menubar-*.png
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
ORIGINALS = ASSETS / "originals"


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
    """Black template. white_is_hole=True → white arrows become cutouts (syncing)."""
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
        idle = svg_png(ORIGINALS / "idle.svg", 512)
        idle.save(tmp / "idle.png")
        magick_ico(tmp / "idle.png", ASSETS / "app-idle.ico", "256,128,64,48,32,16")
        build_appicon(idle)
        print("wrote app-idle.ico, AppIcon.icns")

        frames = []
        for i in range(1, 7):
            im = svg_png(ORIGINALS / f"syncing{i}.svg", 256)
            frames.append(im)
            png = tmp / f"s{i}.png"
            im.save(png)
            if i == 1:
                magick_ico(png, ASSETS / "syncing.ico", "256,128,64,48,32,16")
            magick_ico(png, ASSETS / f"syncing{i + 1}.ico", "64,48,32,16")
        print("wrote syncing.ico + syncing2..7.ico")

        done = svg_png(ORIGINALS / "complete.svg", 256)
        done.save(tmp / "done.png")
        magick_ico(tmp / "done.png", ASSETS / "complete.ico", "256,128,64,48,32,16")
        print("wrote complete.ico")

        # Menubar templates: 44px fills Retina status item better than 22.
        def save_menubar(src: Image.Image, name: str, white_hole: bool) -> None:
            t = to_menubar_template(src, white_is_hole=white_hole)
            t.resize((44, 44), Image.Resampling.LANCZOS).save(ASSETS / name)

        # white_is_hole=True so shield fill stays transparent; outline+check remain.
        # False filled the whole shield solid and hid the checkmark in the menu bar.
        save_menubar(idle, "menubar-icon.png", True)
        save_menubar(frames[0], "menubar-syncing.png", True)
        save_menubar(done, "menubar-complete.png", True)
        print("wrote menubar-*.png (44×44)")

        # Bridge server tile = complete shield on green (from complete.svg)
        svg_png(ORIGINALS / "complete.svg", 32).save(ASSETS / "bridge-server.png")
        if (ASSETS / "bridge-pc.svg").is_file():
            svg_png(ASSETS / "bridge-pc.svg", 40).save(ASSETS / "bridge-pc.png")
        print("wrote bridge-*.png")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("done")


if __name__ == "__main__":
    main()
