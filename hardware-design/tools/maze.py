"""Maze-route the few pads the autorouter and finish_route leave unconnected.

Runs under the KiCad AppImage's python (needs pcbnew), after a DRC report has been
written to /tmp/maze_drc.json. For each unconnected pad in that report it runs a
grid breadth-first search from the pad to the net's existing copper, threading
between neighbouring pin escapes and changing layer through vias where a straight
or L-shaped run cannot get through. Obstacle cells are other-net copper inflated
by the clearance, so additions are DRC-clean by construction.

Usage: <appimage> python3.11 tools/maze.py board.kicad_pcb /tmp/maze_drc.json
"""

import json
import re
import sys
from collections import deque

import pcbnew

RES = 0.1
W, H = 30.0, 40.0
CLR = 0.15
TRACK_W = 0.2
VIA_DIA, VIA_DRILL = 0.6, 0.3
NX, NY = int(W / RES) + 2, int(H / RES) + 2


def disc(cx, cy, rad, into):
    gx, gy = int(round(cx / RES)), int(round(cy / RES))
    r = int(rad / RES) + 1
    for dx in range(-r, r + 1):
        for dy in range(-r, r + 1):
            x, y = gx + dx, gy + dy
            if 0 <= x < NX and 0 <= y < NY and (dx * dx + dy * dy) * RES * RES <= rad * rad:
                into.add((x, y))


def seg(ax, ay, bx, by, rad, into):
    n = max(1, int((((bx - ax) ** 2 + (by - ay) ** 2) ** 0.5) / (RES / 2)))
    for i in range(n + 1):
        t = i / n
        disc(ax + (bx - ax) * t, ay + (by - ay) * t, rad, into)


def grids(board, layer, net):
    """(blocked, own) cell sets on `layer` for the given net code."""
    blocked, own = set(), set()
    mm = pcbnew.ToMM
    half = TRACK_W / 2 + CLR
    for t in board.GetTracks():
        same = t.GetNetCode() == net
        if t.Type() == pcbnew.PCB_VIA_T:
            if t.IsOnLayer(layer):
                p = t.GetPosition()
                if same:
                    disc(mm(p.x), mm(p.y), VIA_DIA / 2, own)
                else:
                    disc(mm(p.x), mm(p.y), VIA_DIA / 2 + half, blocked)
        elif t.IsOnLayer(layer):
            a, e = t.GetStart(), t.GetEnd()
            rad = mm(t.GetWidth()) / 2 + (0 if same else half)
            seg(mm(a.x), mm(a.y), mm(e.x), mm(e.y), rad, own if same else blocked)
    for fp in board.GetFootprints():
        for pad in fp.Pads():
            if not pad.IsOnLayer(layer):
                continue
            p = pad.GetPosition()
            rad = max(mm(pad.GetSizeX()), mm(pad.GetSizeY())) / 2
            if pad.GetNetCode() == net:
                disc(mm(p.x), mm(p.y), rad, own)
            else:
                disc(mm(p.x), mm(p.y), rad + half, blocked)
    return blocked, own


def main() -> None:
    path, drc_path = sys.argv[1], sys.argv[2]
    board = pcbnew.LoadBoard(path)
    f_cu, b_cu = board.GetLayerID("F.Cu"), board.GetLayerID("B.Cu")
    layers = [L for L in (f_cu, board.GetLayerID("In1.Cu"),
                          board.GetLayerID("In2.Cu"), b_cu) if L >= 0]
    li = {L: i for i, L in enumerate(layers)}

    drc = json.load(open(drc_path))
    jobs = []
    for v in drc.get("unconnected_items", []):
        for it in v.get("items", []):
            m = re.search(r"Pad \S+ \[([^\]]+)\] of (\w+)", it.get("description", ""))
            if m:
                jobs.append((m.group(1), it["pos"]["x"], it["pos"]["y"]))

    routed = 0
    for netname, px, py in jobs:
        net = board.FindNet(netname)
        if not net:
            continue
        code = net.GetNetCode()
        blk, own = {}, {}
        for L in layers:
            blk[L], own[L] = grids(board, L, code)
        start = (int(round(px / RES)), int(round(py / RES)), li[f_cu])
        # Target the net's copper, but not the straggler's own little isolated stub
        # near the pad - otherwise we just bond to it and stay unconnected to the rail.
        sgx, sgy = start[0], start[1]
        excl = int(2.2 / RES)
        tgt = {(x, y, li[L]) for L in layers for (x, y) in own[L]
               if abs(x - sgx) > excl or abs(y - sgy) > excl}
        tgt.discard(start)
        prev = {start: None}
        q = deque([start])
        goal = None
        while q:
            cx, cy, cl = q.popleft()
            if (cx, cy, cl) in tgt:
                goal = (cx, cy, cl)
                break
            nbrs = [(cx + 1, cy, cl), (cx - 1, cy, cl), (cx, cy + 1, cl), (cx, cy - 1, cl),
                    (cx, cy, (cl + 1) % len(layers)), (cx, cy, (cl - 1) % len(layers))]
            for ncx, ncy, ncl in nbrs:
                if not (0 <= ncx < NX and 0 <= ncy < NY) or (ncx, ncy, ncl) in prev:
                    continue
                L = layers[ncl]
                if (ncx, ncy) in blk[L] and (ncx, ncy, ncl) not in tgt:
                    continue
                if ncl != cl and any((cx, cy) in blk[layers[i]] for i in range(len(layers))):
                    continue
                prev[(ncx, ncy, ncl)] = (cx, cy, cl)
                q.append((ncx, ncy, ncl))
        if not goal:
            print(f"  maze: no path for {netname} @ ({px},{py})")
            continue
        seq = []
        s = goal
        while s is not None:
            seq.append(s)
            s = prev[s]
        seq.reverse()
        # Stub from the exact pad centre to the first grid cell so it bonds cleanly.
        sx0, sy0, sl0 = seq[0]
        st = pcbnew.PCB_TRACK(board)
        st.SetStart(pcbnew.VECTOR2I(pcbnew.FromMM(px), pcbnew.FromMM(py)))
        st.SetEnd(pcbnew.VECTOR2I(pcbnew.FromMM(sx0 * RES), pcbnew.FromMM(sy0 * RES)))
        st.SetWidth(pcbnew.FromMM(TRACK_W))
        st.SetLayer(layers[sl0])
        st.SetNetCode(code)
        board.Add(st)
        for (ax, ay, al), (bx, by, bl) in zip(seq, seq[1:]):
            if al != bl:
                v = pcbnew.PCB_VIA(board)
                v.SetPosition(pcbnew.VECTOR2I(pcbnew.FromMM(ax * RES), pcbnew.FromMM(ay * RES)))
                v.SetDrill(pcbnew.FromMM(VIA_DRILL))
                v.SetWidth(pcbnew.FromMM(VIA_DIA))
                v.SetNetCode(code)
                v.SetLayerPair(f_cu, b_cu)
                board.Add(v)
            else:
                t = pcbnew.PCB_TRACK(board)
                t.SetStart(pcbnew.VECTOR2I(pcbnew.FromMM(ax * RES), pcbnew.FromMM(ay * RES)))
                t.SetEnd(pcbnew.VECTOR2I(pcbnew.FromMM(bx * RES), pcbnew.FromMM(by * RES)))
                t.SetWidth(pcbnew.FromMM(TRACK_W))
                t.SetLayer(layers[al])
                t.SetNetCode(code)
                board.Add(t)
        routed += 1

    pcbnew.SaveBoard(path, board)
    print(f"maze: routed {routed}/{len(jobs)} stragglers")


if __name__ == "__main__":
    main()
