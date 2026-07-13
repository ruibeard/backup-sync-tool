#!/usr/bin/env python3
"""Generate all icons from shield SVG masters (one metaphor).

Masters:
  assets/idle.svg          green outline shield + check
  assets/syncing1..6.svg   blue filled shield + white spin arrows
  assets/syncing.svg       = syncing1
  assets/complete.svg      green filled shield + white check

Derived: app-idle.ico, AppIcon.icns, syncing*.ico, complete.ico, menubar-*.png
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
        idle = svg_png(ASSETS / "idle.svg", 512)
        idle.save(tmp / "idle.png")
        # Also refresh brand.png from idle SVG for consistency
        idle.save(ASSETS / "brand.png")
        magick_ico(tmp / "idle.png", ASSETS / "app-idle.ico", "256,128,64,48,32,16")
        build_appicon(idle)
        print("wrote idle → app-idle.ico, AppIcon.icns, brand.png")

        frames = []
        for i in range(1, 7):
            im = svg_png(ASSETS / f"syncing{i}.svg", 256)
            frames.append(im)
            png = tmp / f"s{i}.png"
            im.save(png)
            if i == 1:
                magick_ico(png, ASSETS / "syncing.ico", "256,128,64,48,32,16")
            magick_ico(png, ASSETS / f"syncing{i + 1}.ico", "64,48,32,16")
        print("wrote syncing.ico + syncing2..7.ico (shield + spin)")

        done = svg_png(ASSETS / "complete.svg", 256)
        done.save(tmp / "done.png")
        magick_ico(tmp / "done.png", ASSETS / "complete.ico", "256,128,64,48,32,16")
        print("wrote complete.ico")

        # Menubar: idle keeps white fill solid; syncing/complete punch white holes
        to_menubar_template(idle, white_is_hole=False).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-icon.png")
        to_menubar_template(frames[0], white_is_hole=True).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-syncing.png")
        to_menubar_template(done, white_is_hole=True).resize(
            (22, 22), Image.Resampling.LANCZOS
        ).save(ASSETS / "menubar-complete.png")
        print("wrote menubar-*.png")

        # Bridge: plate = filled green shield style
        plate = svg_png(ASSETS / "complete.svg", 32)
        # solid green square + white shield like old brand-plate — use idle on green
        bg = Image.new("RGBA", (32, 32), (0x01, 0x66, 0x30, 255))
        # white stroke shield from idle silhouette
        mark = to_menubar_template(
            svg_png(ASSETS / "idle.svg", 24), white_is_hole=False
        )
        # invert to white
        wmark = Image.new("RGBA", mark.size, (0, 0, 0, 0))
        sp, dp = mark.load(), wmark.load()
        for y in range(mark.size[1]):
            for x in range(mark.size[0]):
                _r, _g, _b, a = sp[x, y]
                if a >= 12:
                    dp[x, y] = (255, 255, 255, a)
        ox = (32 - wmark.size[0]) // 2
        oy = (32 - wmark.size[1]) // 2
        bg.alpha_composite(wmark, (ox, oy))
        bg.save(ASSETS / "bridge-server.png")
        bg.save(ASSETS / "brand-plate.png")

        if (ASSETS / "bridge-pc.svg").is_file():
            svg_png(ASSETS / "bridge-pc.svg", 40).save(ASSETS / "bridge-pc.png")
        print("wrote bridge-*.png")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("done — all icons are shields")


if __name__ == "__main__":
    main()
