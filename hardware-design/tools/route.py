"""Route the placed board end to end: autoroute, finish, then pour ground.

Pipeline: export Specctra DSN (pcbnew) -> freerouting (headless) -> import SES
(pcbnew) -> close stragglers (finish_route) -> ground pour (pour). Each step is a
separate tool so they can be run by hand; this just orchestrates them. The board
must already be placed (tools/build_board.py).

Requires the KiCad AppImage (for the bundled pcbnew) and the freerouting jar.
Run from anywhere: python3 tools/route.py [--passes N]
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PCB = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_pcb"
DSN, SES = Path("/tmp/node.dsn"), Path("/tmp/node.ses")
APPIMAGE = Path(os.environ.get("KICAD_APPIMAGE", Path.home() / ".local/bin/kicad-10.0.3-x86_64.AppImage"))
FREEROUTING = Path(os.environ.get("FREEROUTING_JAR", Path.home() / ".local/share/freerouting/freerouting-2.2.4.jar"))
TOOLS = ROOT / "tools"


def kpython(script: str, *args) -> None:
    subprocess.run([str(APPIMAGE), "python3.11", str(TOOLS / script), *map(str, args)], check=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--passes", type=int, default=100, help="freerouting optimisation passes")
    args = ap.parse_args()
    for tool in (APPIMAGE, FREEROUTING):
        if not tool.exists():
            sys.exit(f"missing {tool} (set KICAD_APPIMAGE / FREEROUTING_JAR)")

    print("1/5 export DSN")
    kpython("pcb_dsn.py", PCB, DSN)

    print("2/5 freerouting signals (GND left for the pour)")
    subprocess.run(
        ["java", "-Djava.awt.headless=true", "-jar", str(FREEROUTING),
         "--gui.enabled=false", "-de", str(DSN), "-do", str(SES),
         "-mp", str(args.passes), "-inc", "GND", "-da"],
        check=True,
    )

    print("3/5 import SES")
    kpython("pcb_ses.py", PCB, SES)

    print("4/5 finish stragglers")
    kpython("finish_route.py", PCB)

    print("5/5 ground pour + stitching")
    kpython("pour.py", PCB)
    print("routing complete")


if __name__ == "__main__":
    main()
