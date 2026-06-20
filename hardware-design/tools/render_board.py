"""Photorealistic 3D renders of the populated board -> diagrams/renders/.

Runs `kicad-cli pcb render` (raytraced, with the bundled 3D models) under a virtual
framebuffer so it works headless. Produces a straight-down top view, an angled 3D
view and a bottom view. This is the final step of the schematic-to-board pipeline:
a quick visual confirmation that the populated board still looks right after an edit.

Usage: python3 tools/render_board.py
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PCB = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_pcb"
OUT = ROOT / "diagrams" / "renders"

VIEWS = [
    ("board-top.png", ["--side", "top", "--width", "1800", "--height", "2200"]),
    ("board-bottom.png", ["--side", "bottom", "--width", "1800", "--height", "2200"]),
    ("board-3d.png", ["--side", "top", "--perspective", "--rotate", "-30,0,25",
                      "--zoom", "0.9", "--width", "2000", "--height", "1600"]),
]


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    xvfb = shutil.which("xvfb-run")
    if not (xvfb and shutil.which("kicad-cli")):
        print("render skipped: need xvfb-run and kicad-cli")
        return
    for name, args in VIEWS:
        cmd = [xvfb, "-a", "kicad-cli", "pcb", "render", "--quality", "high",
               "--background", "opaque", *args, "-o", str(OUT / name), str(PCB)]
        print(f"+ render {name}")
        subprocess.run(cmd, check=True)
    print(f"renders in {OUT}")


if __name__ == "__main__":
    main()
