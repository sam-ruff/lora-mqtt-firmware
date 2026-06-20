"""Import a Specctra SES session into a .kicad_pcb using the bundled pcbnew.

Applies a freerouting result (tracks + vias) back onto the board and saves it.
Run under the KiCad AppImage's python. Used by tools/route.py.

Usage: <appimage> python3.11 tools/pcb_ses.py board.kicad_pcb session.ses
"""

import sys

import pcbnew

board_path, ses_path = sys.argv[1], sys.argv[2]
board = pcbnew.LoadBoard(board_path)
if not pcbnew.ImportSpecctraSES(board, ses_path):
    raise SystemExit("SES import failed")
pcbnew.SaveBoard(board_path, board)
tracks = board.GetTracks()
n_via = sum(1 for t in tracks if t.Type() == pcbnew.PCB_VIA_T)
print(f"imported {ses_path}: {len(tracks) - n_via} track segments, {n_via} vias")
