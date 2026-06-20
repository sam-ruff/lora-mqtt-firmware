"""Placement overrides applied on top of the manufactured pick-and-place file.

The CPL positions were drawn for the original 4-layer board with its exact
footprints. A few substitute footprints (the larger USB-C receptacle, the module)
are physically bigger, so at the CPL coordinates they overlap their neighbours.
Each entry here is (x_mm, y_mm, rotation_deg) in board coordinates and wins over
the CPL for that designator; every part stays on the top side.

Workflow: edit a value, run `python3 tools/build_board.py`, then
`python3 tools/check_placement.py` until it reports no overlaps.
"""

from __future__ import annotations

# Locked pre-routes drawn (by tools/preroute.py) before the autorouter runs, to
# claim a lane for a pin freerouting otherwise boxes in. Each is
# (net, (ref, pad), (ref, pad)); a locked F.Cu track joins the two pads so the
# autorouter routes everything else around it.
PREROUTES: list[tuple[str, tuple[str, str], tuple[str, str]]] = [
    ("+3V3", ("U3", "46"), ("C8", "1")),   # +3V3 CPU pin, hemmed in by LoRa escapes
]

OVERRIDES: dict[str, tuple[float, float, float]] = {
    # USB-C cluster: the substitute receptacle is wider than the original, so pull
    # it flush to the edge and move the ESD device + CC resistor clear of its body.
    "J1": (24.6, 20.0, 90.0),
    "U1": (17.8, 21.0, -90.0),
    "R1": (22.0, 27.0, 0.0),
    # C7 (MCU decoupling) parks in the open channel right of U3, clear of U1/J1.
    "C7": (17.7, 16.0, 90.0),
    # Crystal hard against the ESP32 XTAL pins (U3 left edge ~x8.55) so XTAL_P/N
    # are short and uncongested.
    "Y1": (5.7, 20.6, 0.0),
    # Crystal load caps clear of the moved crystal.
    "C20": (3.0, 23.2, 0.0),
    # A +3V3 decoupling cap right at the +3V3 pin group on U3's left edge; rotated
    # 180 so its +3V3 pad faces pin 46 for the locked pre-route below.
    "C8": (6.0, 17.6, 180.0),
    # RF match at the ESP32 antenna pin (U3 pin 1 ~9.4,22.45), in the clear strip
    # below U3, so ESP_ANT is short; the matched ANT1 line runs out to the U.FL.
    "C15": (8.6, 24.8, 90.0),
    "L2": (7.0, 24.5, 90.0),
    "L1": (5.5, 24.5, 90.0),
    # Flash cluster: clear the output cap and the NRST pull-up off the SOIC body.
    "C19": (26.5, 28.0, 90.0),
    "R7": (13.0, 25.5, 0.0),
}
