"""Place board footprints at the manufactured coordinates from the CPL file.

Run AFTER the schematic netlist has been imported to the board, so the footprints
exist. Each board footprint is matched by reference designator to a row in the
pick-and-place CSV and its position, rotation and side are set to match the
as-manufactured board. Coordinates share the convention used by tools/board_setup.py.
"""

from __future__ import annotations

import csv
from pathlib import Path

from kiutils.board import Board

ROOT = Path(__file__).resolve().parent.parent
PCB = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_pcb"
CPL = ROOT / "Production Files" / "Pick and Place" / "MESHTASTIC NODE-CPL.csv"


def load_cpl(path: Path) -> dict[str, tuple[float, float, float, str]]:
    rows: dict[str, tuple[float, float, float, str]] = {}
    with path.open(newline="") as fh:
        for r in csv.DictReader(fh):
            ref = r["Designator"].strip('"')
            rows[ref] = (
                float(r["Mid X"]), float(r["Mid Y"]),
                float(r["Rotation"]), r["Layer"].strip('"'),
            )
    return rows


def footprint_ref(fp) -> str | None:
    """Reference designator across kiutils footprint layouts (dict/list/FpText)."""
    props = getattr(fp, "properties", None)
    if isinstance(props, dict):
        return props.get("Reference")
    if isinstance(props, list):
        for p in props:
            if getattr(p, "key", None) == "Reference":
                return p.value
    for gi in getattr(fp, "graphicItems", []):
        if getattr(gi, "type", None) == "reference":
            return getattr(gi, "text", None)
    return None


def main() -> None:
    board = Board.from_file(str(PCB))
    cpl = load_cpl(CPL)
    if not board.footprints:
        print("no footprints on the board yet - import the schematic netlist first")
        return

    placed, missing = 0, []
    for fp in board.footprints:
        ref = footprint_ref(fp)
        if ref in cpl:
            x, y, rot, layer = cpl[ref]
            fp.position.X, fp.position.Y, fp.position.angle = x, y, rot
            fp.layer = "F.Cu" if layer.lower() == "top" else "B.Cu"
            placed += 1
        else:
            missing.append(ref)

    board.to_file(str(PCB))
    print(f"placed {placed} footprints from the CPL")
    if missing:
        print("no CPL row for:", ", ".join(str(m) for m in missing if m))


if __name__ == "__main__":
    main()
