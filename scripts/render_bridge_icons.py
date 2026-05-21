"""Render H5 bridge icons for the Win32 app.

Both tiles are rasterized from SVG via resvg at 6x display size (240px) for
crisp Lanczos downscaling in the app. Server tile uses the project shield icon
(bridge-server.svg). Connection state is drawn beside the Server label.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets"
SVG_PC = OUT / "bridge-pc.svg"
SVG_SERVER = OUT / "bridge-server.svg"

DISPLAY_SIZE = 40
RENDER_SIZE = 240


def render_svg_tile(svg: Path, out_name: str) -> None:
    if not svg.exists():
        raise FileNotFoundError(svg)

    out_path = OUT / out_name
    tmp = OUT / f"_{out_name}"
    cmd = [
        "resvg",
        "-w",
        str(RENDER_SIZE),
        "-h",
        str(RENDER_SIZE),
        str(svg),
        str(tmp),
    ]
    try:
        subprocess.run(cmd, check=True, capture_output=True, text=True)
    except FileNotFoundError as exc:
        raise RuntimeError("resvg CLI not found; install with: pip install resvg-cli") from exc
    except subprocess.CalledProcessError as exc:
        raise RuntimeError(f"resvg failed: {exc.stderr or exc.stdout}") from exc

    tmp.replace(out_path)
    print(f"wrote {out_path} ({RENDER_SIZE}x{RENDER_SIZE}) from {svg.name}")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    render_svg_tile(SVG_PC, "bridge-pc.png")
    render_svg_tile(SVG_SERVER, "bridge-cloud.png")


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(err, file=sys.stderr)
        sys.exit(1)
