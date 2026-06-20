"""Export a Specctra DSN from a .kicad_pcb using the bundled pcbnew.

kicad-cli cannot export Specctra, so this runs under the KiCad AppImage's python
(which bundles pcbnew). Used by tools/route.py.

Usage: <appimage> python3.11 tools/pcb_dsn.py board.kicad_pcb out.dsn
"""

import sys

import pcbnew

board = pcbnew.LoadBoard(sys.argv[1])
if not pcbnew.ExportSpecctraDSN(board, sys.argv[2]):
    raise SystemExit("DSN export failed")
print(f"wrote {sys.argv[2]}: {len(board.GetFootprints())} footprints, {board.GetNetCount()} nets")
