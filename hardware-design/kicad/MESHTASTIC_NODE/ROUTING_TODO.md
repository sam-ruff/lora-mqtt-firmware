# Three connections to finish by hand

The board is DRC-clean (0 errors, 0 warnings, 0 schematic-parity) except for
three connections the free autorouter and the headless finishers could not
thread through the dense QFN pin escapes. Close them in the KiCad GUI's
interactive router, then re-run the fab/render steps. Total work is a few
minutes.

Open the board:

```bash
kicad kicad/MESHTASTIC_NODE/MESHTASTIC_NODE.kicad_pcb
```

In the PCB editor: press `X` for the interactive router, click the start pad,
click the end point. The router walks-around and pushes neighbouring tracks,
which is exactly what the headless tools cannot do. Leave it on F.Cu where it
fits; drop to an inner/bottom layer with `V` (places a via) if a run is blocked.
DRC (`Inspect -> Design Rules Checker`) must stay at 0 after each.

All coordinates are board mm (top-left origin).

1. **+3V3 -> U3 pad 46** (VDD3P3_CPU), pad at (8.55, 17.60) on F.Cu.
   Nearest existing +3V3 copper is at (6.78, 17.60) on F.Cu, 1.78 mm to the
   left - a near-straight horizontal run.

2. **+3V3 -> U3 pad 20** (VDD3P3_RTC), pad at (15.45, 19.60) on F.Cu.
   Nearest +3V3 copper is at (17.70, 16.77) on F.Cu, 3.61 mm up-right.

3. **FLASH_WP**: U3 pad 31 at (13.80, 15.55) <-> U4 pad 3 at (26.10, 32.37),
   both F.Cu. This one is a longer diagonal across the board; route it on B.Cu
   (via down at each end) to stay clear of the top-side signals, or let the
   interactive router push through on F.Cu.

After routing, finish the package:

```bash
python3 tools/build_all.py --skip-route   # re-pour, DRC, Gerbers, BOM, renders
```

`--skip-route` keeps your hand-routing and only re-checks, re-pours ground,
re-exports `kicad/fab/`, and re-renders. DRC should now report 0 unconnected.
