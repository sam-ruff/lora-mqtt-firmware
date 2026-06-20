"""Generate the full manufacturing package (Gerbers, drill, CPL, BOM) for PCBWay.

Run after the board is routed and poured (tools/route.py). Produces, under
kicad/fab/:
  - gerbers/   RS-274X Gerbers (4-layer) + Excellon drill, zone fill refreshed
  - gerbers.zip   the Gerber+drill set, ready to upload
  - CPL.csv    pick-and-place (top side; this board is single-sided)
  - BOM.csv    assembly BOM grouped by value, with LCSC part numbers
  - netlist.net
Upload gerbers.zip plus BOM.csv and CPL.csv to PCBWay.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KP = ROOT / "kicad" / "MESHTASTIC_NODE"
SCH = KP / "MESHTASTIC_NODE.kicad_sch"
PCB = KP / "MESHTASTIC_NODE.kicad_pcb"
OUT = ROOT / "kicad" / "fab"
GERB = OUT / "gerbers"

GERBER_LAYERS = ("F.Cu,In1.Cu,In2.Cu,B.Cu,"
                 "F.Paste,B.Paste,F.SilkS,B.SilkS,F.Mask,B.Mask,Edge.Cuts")


def run(*args) -> None:
    cmd = [str(a) for a in args]
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True)


def main() -> None:
    if GERB.exists():
        shutil.rmtree(GERB)
    GERB.mkdir(parents=True, exist_ok=True)

    # Assembly BOM (grouped) + netlist from the schematic.
    run("kicad-cli", "sch", "export", "bom", "--group-by", "Value",
        "--fields", "Reference,Value,Footprint,LCSC,${QUANTITY}",
        "--labels", "Designator,Comment,Footprint,LCSC,Qty",
        "-o", OUT / "BOM.csv", SCH)
    run("kicad-cli", "sch", "export", "netlist", "-o", OUT / "netlist.net", SCH)

    # Gerbers (refill zones at plot time) + Excellon drill with map.
    run("kicad-cli", "pcb", "export", "gerbers", "--check-zones", "--no-protel-ext",
        "--layers", GERBER_LAYERS, "-o", str(GERB) + "/", PCB)
    run("kicad-cli", "pcb", "export", "drill", "--format", "excellon",
        "--drill-origin", "absolute", "--excellon-units", "mm",
        "--generate-map", "--map-format", "gerberx2", "-o", str(GERB) + "/", PCB)

    # Pick-and-place (top only - all parts are on the top side).
    run("kicad-cli", "pcb", "export", "pos", "--format", "csv", "--units", "mm",
        "--side", "front", "-o", OUT / "CPL.csv", PCB)

    # Zip the Gerber + drill set for upload.
    archive = shutil.make_archive(str(OUT / "gerbers"), "zip", root_dir=GERB)
    print(f"\nfab package in {OUT}\n  gerbers: {len(list(GERB.iterdir()))} files"
          f"\n  archive: {archive}")


if __name__ == "__main__":
    main()
