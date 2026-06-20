"""Draw the locked pre-route tracks from placement.PREROUTES.

Runs under the KiCad AppImage's python (needs pcbnew), after build_board.py and
before the DSN export, so the autorouter sees these tracks as fixed wiring and
routes everything else around them. Used for pins the autorouter otherwise boxes
in (e.g. an ESP32 power pin surrounded by the LoRa pin escapes).

Usage: <appimage> python3.11 tools/preroute.py board.kicad_pcb
"""

import sys

import pcbnew

import placement


def main() -> None:
    path = sys.argv[1]
    board = pcbnew.LoadBoard(path)
    f_cu = board.GetLayerID("F.Cu")
    pads = {(fp.GetReference(), p.GetName()): p
            for fp in board.GetFootprints() for p in fp.Pads()}

    n = 0
    for net, a, b in placement.PREROUTES:
        pa, pb = pads.get(a), pads.get(b)
        if not (pa and pb):
            print(f"  pre-route skipped, pad missing: {a} or {b}")
            continue
        t = pcbnew.PCB_TRACK(board)
        t.SetStart(pa.GetPosition())
        t.SetEnd(pb.GetPosition())
        t.SetWidth(pcbnew.FromMM(0.25))
        t.SetLayer(f_cu)
        t.SetNetCode(board.FindNet(net).GetNetCode())
        t.SetLocked(True)
        board.Add(t)
        n += 1
    pcbnew.SaveBoard(path, board)
    print(f"preroute: {n} locked tracks")


if __name__ == "__main__":
    main()
