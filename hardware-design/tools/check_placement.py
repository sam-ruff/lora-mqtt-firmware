"""Report placement problems on the board: courtyard overlaps and edge clearance.

A fast, scriptable pre-routing check that complements `kicad-cli pcb drc`. For
every footprint it builds the courtyard bounding box (transformed by the part's
position and rotation) and reports any pair whose courtyards overlap, plus any
part whose courtyard pokes outside the board outline or sits within EDGE_GAP of
it. Use it to drive placement edits until it prints "no overlaps".
"""

from __future__ import annotations

import math
from pathlib import Path

from kiutils.board import Board

ROOT = Path(__file__).resolve().parent.parent
PCB = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_pcb"

# Board outline (matches board_setup.py): 30 x 40 mm rectangle.
BOARD_W, BOARD_H = 30.0, 40.0
EDGE_GAP = 0.3  # min courtyard-to-edge gap we want before routing


def rotate(px: float, py: float, deg: float) -> tuple[float, float]:
    """Rotate (px, py) by the footprint angle, in KiCad's Y-down convention.

    Verified against DRC: a +90 deg part maps local (x, y) to board (y, -x).
    """
    r = math.radians(deg)
    c, s = math.cos(r), math.sin(r)
    return px * c + py * s, -px * s + py * c


def courtyard_bbox(fp) -> tuple[float, float, float, float] | None:
    """Axis-aligned bbox of the footprint courtyard in board coordinates."""
    fx, fy = fp.position.X, fp.position.Y
    ang = fp.position.angle or 0
    pts: list[tuple[float, float]] = []
    for g in fp.graphicItems:
        if getattr(g, "layer", None) not in ("F.CrtYd", "B.CrtYd"):
            continue
        for attr in ("start", "end", "center", "mid"):
            p = getattr(g, attr, None)
            if p is None:
                continue
            rx, ry = rotate(p.X, p.Y, ang)
            pts.append((fx + rx, fy + ry))
        # Polygon courtyards (e.g. USB-C) carry their points in `coordinates`.
        for p in getattr(g, "coordinates", None) or []:
            rx, ry = rotate(p.X, p.Y, ang)
            pts.append((fx + rx, fy + ry))
    if not pts:
        return None
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    return min(xs), min(ys), max(xs), max(ys)


def overlap(a, b) -> float:
    """Overlap area of two bboxes (0 if disjoint)."""
    ox = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    oy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    return ox * oy


def main() -> int:
    board = Board.from_file(str(PCB))
    boxes: dict[str, tuple] = {}
    seen: list[str] = []
    for fp in board.footprints:
        ref = fp.properties.get("Reference") if isinstance(fp.properties, dict) else None
        bb = courtyard_bbox(fp)
        if ref and bb:
            seen.append(ref)
            boxes[ref] = bb
    dupes = sorted({r for r in seen if seen.count(r) > 1})
    if dupes:
        print(f"WARNING: duplicate footprints on board (re-run board_setup): {dupes}")

    refs = sorted(boxes)
    pairs = []
    for i, a in enumerate(refs):
        for b in refs[i + 1:]:
            ov = overlap(boxes[a], boxes[b])
            if ov > 1e-4:
                pairs.append((ov, a, b))
    pairs.sort(reverse=True)

    edge = []
    for ref, (x0, y0, x1, y1) in boxes.items():
        gap = min(x0, y0, BOARD_W - x1, BOARD_H - y1)
        if gap < EDGE_GAP:
            edge.append((gap, ref))
    edge.sort()

    print(f"footprints with courtyards: {len(boxes)}")
    print(f"\n== courtyard overlaps: {len(pairs)} ==")
    for ov, a, b in pairs:
        print(f"  {a:5s} <> {b:5s}  overlap {ov:.2f} mm^2")
    print(f"\n== edge clearance < {EDGE_GAP} mm: {len(edge)} ==")
    for gap, ref in edge:
        print(f"  {ref:5s}  gap {gap:+.2f} mm")
    return 1 if (pairs or edge) else 0


if __name__ == "__main__":
    raise SystemExit(main())
