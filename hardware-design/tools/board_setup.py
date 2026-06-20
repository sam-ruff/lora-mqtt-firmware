"""Create the 2-layer board (F.Cu / B.Cu) with the Edge.Cuts outline.

The outline is reproduced from the manufacturing Gerber (Edge_Cuts.gm1): a
30 x 40 mm board with 3 mm rounded corners on all four corners and a USB-C
connector notch on the right edge (x 26.39-30 mm, y 15.8-24.2 mm). Coordinates use
the same convention as the pick-and-place file so footprints placed from the CPL
line up with this outline.
"""

from __future__ import annotations

import math
import uuid
from pathlib import Path

from kiutils.board import Board
from kiutils.items.brditems import LayerToken
from kiutils.items.common import Position
from kiutils.items.gritems import GrArc, GrLine

PCB = (
    Path(__file__).resolve().parent.parent
    / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_pcb"
)
EDGE = "Edge.Cuts"
W = 0.1


def line(x1: float, y1: float, x2: float, y2: float) -> GrLine:
    return GrLine(
        start=Position(x1, y1), end=Position(x2, y2),
        layer=EDGE, width=W, tstamp=str(uuid.uuid4()),
    )


def arc(cx: float, cy: float, a_start: float, a_end: float) -> GrArc:
    """Quarter-circle arc of radius 3 about (cx, cy), angles in degrees."""
    r = 3.0
    a_mid = (a_start + a_end) / 2
    pt = lambda a: Position(cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
    return GrArc(start=pt(a_start), mid=pt(a_mid), end=pt(a_end),
                 layer=EDGE, width=W, tstamp=str(uuid.uuid4()))


def main() -> None:
    b = Board().create_new()
    # 4-layer stack: F.Cu / In1.Cu / In2.Cu / B.Cu. Inner layers are signal-typed so
    # the dense design can route on all four; tools/pour.py floods every copper layer
    # with a stitched ground (matching the original's ground-rich inner layers).
    b.layers.insert(1, LayerToken(ordinal=1, name="In1.Cu", type="signal"))
    b.layers.insert(2, LayerToken(ordinal=2, name="In2.Cu", type="signal"))

    outline = [
        line(3, 0, 27, 0),            # bottom
        arc(27, 3, -90, 0),           # bottom-right corner
        # Straight right edge: the substitute SMD USB-C (J1) mounts on solid board
        # with its mating face at the edge, so it needs no connector notch.
        line(30, 3, 30, 37),          # right edge
        arc(27, 37, 0, 90),           # top-right corner
        line(27, 40, 3, 40),          # top
        arc(3, 37, 90, 180),          # top-left corner
        line(0, 37, 0, 3),            # left edge
        arc(3, 3, 180, 270),          # bottom-left corner
    ]
    b.graphicItems.extend(outline)

    b.to_file(str(PCB))
    print(f"layers: {[l.name for l in b.layers if l.type == 'signal']}")
    print(f"edge items: {sum(1 for g in b.graphicItems if getattr(g, 'layer', None) == EDGE)}")
    print(f"wrote {PCB}")


if __name__ == "__main__":
    main()
