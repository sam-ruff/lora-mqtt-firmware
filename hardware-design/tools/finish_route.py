"""Close the last few connections the autorouter could not finish.

Runs under the KiCad AppImage's python (needs pcbnew). For every signal/power net
it builds connectivity clusters from the existing pads, tracks and vias, then for
each pad left out of the main cluster it adds a short connecting route, trying a
direct F.Cu segment first and a via -> B.Cu -> via detour second, accepting only a
path that clears all other-net copper. Deterministic, so it runs as a normal
pipeline step. Vias/zones for GND are left to tools/pour.py.

Usage: <appimage> python3.11 tools/pcb_finish.py board.kicad_pcb
"""

import math
import sys

import pcbnew

TOL = pcbnew.FromMM(0.06)        # endpoints closer than this are one node
CLEAR = pcbnew.FromMM(0.18)      # required gap to other-net copper (>0.13 rule)
TRACK_W = pcbnew.FromMM(0.2)
VIA_DIA, VIA_DRILL = pcbnew.FromMM(0.6), pcbnew.FromMM(0.3)
SKIP_NETS = {"GND"}             # GND is handled by the pour


def seg_pt_dist(ax, ay, bx, by, px, py):
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return ((px - ax) ** 2 + (py - ay) ** 2) ** 0.5
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    cx, cy = ax + t * dx, ay + t * dy
    return ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5


def seg_seg_dist(a, b, c, d):
    return min(
        seg_pt_dist(*a, *b, *c), seg_pt_dist(*a, *b, *d),
        seg_pt_dist(*c, *d, *a), seg_pt_dist(*c, *d, *b),
    )


class Obstacles:
    """Other-net copper per layer as segments (tracks) and discs (pads/vias)."""

    def __init__(self, board, layer):
        self.segs, self.discs = [], []
        self.layer = layer
        for t in board.GetTracks():
            if t.Type() == pcbnew.PCB_VIA_T:
                if t.IsOnLayer(layer):
                    p = t.GetPosition()
                    self.discs.append((p.x, p.y, t.GetWidth() / 2, t.GetNetCode()))
            elif t.IsOnLayer(layer):
                a, b = t.GetStart(), t.GetEnd()
                self.segs.append(((a.x, a.y), (b.x, b.y), t.GetWidth() / 2, t.GetNetCode()))
        for fp in board.GetFootprints():
            for pad in fp.Pads():
                if not pad.IsOnLayer(layer):
                    continue
                p = pad.GetPosition()
                w, h, n = pad.GetSizeX(), pad.GetSizeY(), pad.GetNetCode()
                # Model an elongated pad (e.g. a QFN finger) as a segment along its
                # long axis with half-width radius - a disc of the long dimension
                # would wrongly block the clear lanes between fine-pitch pads.
                if abs(w - h) < pcbnew.FromMM(0.05):
                    self.discs.append((p.x, p.y, min(w, h) / 2, n))
                else:
                    ang = math.radians(pad.GetOrientationDegrees())
                    half = abs(w - h) / 2
                    if w > h:
                        dx, dy, r = math.cos(ang) * half, -math.sin(ang) * half, h / 2
                    else:
                        dx, dy, r = math.sin(ang) * half, math.cos(ang) * half, w / 2
                    self.segs.append(((p.x - dx, p.y - dy), (p.x + dx, p.y + dy), r, n))

    def point_clear(self, x, y, net, extra=0.0) -> bool:
        """True if a via/point at (x,y) clears all other-net copper on this layer."""
        need = CLEAR + VIA_DIA / 2 + extra
        for (sx, sy), (ex, ey), hw, n in self.segs:
            if n != net and seg_pt_dist(sx, sy, ex, ey, x, y) < need + hw:
                return False
        for cx, cy, r, n in self.discs:
            if n != net and ((cx - x) ** 2 + (cy - y) ** 2) ** 0.5 < need + r:
                return False
        return True

    def clear(self, ax, ay, bx, by, net) -> bool:
        need = CLEAR + TRACK_W / 2
        for (sx, sy), (ex, ey), hw, n in self.segs:
            if n == net:
                continue
            if seg_seg_dist((ax, ay), (bx, by), (sx, sy), (ex, ey)) < need + hw:
                return False
        for cx, cy, r, n in self.discs:
            if n == net:
                continue
            if seg_pt_dist(ax, ay, bx, by, cx, cy) < need + r:
                return False
        return True


def clusters_for(board, code):
    nodes = []   # each: list of (x, y) endpoints that are electrically one
    for fp in board.GetFootprints():
        for pad in fp.Pads():
            if pad.GetNetCode() == code:
                p = pad.GetPosition()
                nodes.append({"pts": [(p.x, p.y)], "pad": pad, "ref": fp.GetReference()})
    for t in board.GetTracks():
        if t.GetNetCode() != code:
            continue
        if t.Type() == pcbnew.PCB_VIA_T:
            p = t.GetPosition()
            nodes.append({"pts": [(p.x, p.y)], "pad": None})
        else:
            a, b = t.GetStart(), t.GetEnd()
            nodes.append({"pts": [(a.x, a.y), (b.x, b.y)], "pad": None})
    parent = list(range(len(nodes)))

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i in range(len(nodes)):
        for j in range(i + 1, len(nodes)):
            if any(abs(x1 - x2) <= TOL and abs(y1 - y2) <= TOL
                   for x1, y1 in nodes[i]["pts"] for x2, y2 in nodes[j]["pts"]):
                parent[find(i)] = find(j)
    groups = {}
    for i in range(len(nodes)):
        groups.setdefault(find(i), []).append(i)
    return nodes, list(groups.values())


def add_track(board, ax, ay, bx, by, layer, code):
    t = pcbnew.PCB_TRACK(board)
    t.SetStart(pcbnew.VECTOR2I(int(ax), int(ay)))
    t.SetEnd(pcbnew.VECTOR2I(int(bx), int(by)))
    t.SetWidth(TRACK_W)
    t.SetLayer(layer)
    t.SetNetCode(code)
    board.Add(t)


def add_via(board, x, y, code, f, b):
    v = pcbnew.PCB_VIA(board)
    v.SetPosition(pcbnew.VECTOR2I(int(x), int(y)))
    v.SetDrill(VIA_DRILL)
    v.SetWidth(VIA_DIA)
    v.SetNetCode(code)
    v.SetLayerPair(f, b)
    board.Add(v)


MERGE = pcbnew.FromMM(0.4)       # same-net endpoints this close are just merged


def main() -> None:
    path = sys.argv[1]
    board = pcbnew.LoadBoard(path)
    f_cu, b_cu = board.GetLayerID("F.Cu"), board.GetLayerID("B.Cu")
    # Obstacle sets for every copper layer, so a via detour never clips an inner
    # signal on a 4-layer board.
    cu = [board.GetLayerID(n) for n in ("F.Cu", "In1.Cu", "In2.Cu", "B.Cu")
          if board.GetLayerID(n) >= 0]
    obst = {layer: Obstacles(board, layer) for layer in cu}
    obstF, obstB = obst[f_cu], obst[b_cu]
    added = 0

    for net in board.GetNetsByName().values():
        name = net.GetNetname()
        code = net.GetNetCode()
        if code == 0 or name in SKIP_NETS:
            continue
        nodes, groups = clusters_for(board, code)
        if len(groups) <= 1:
            continue
        groups.sort(key=len, reverse=True)
        main_pts = [p for gi in groups[0] for p in nodes[gi]["pts"]]
        for g in groups[1:]:
            src = [p for gi in g for p in nodes[gi]["pts"]]
            # Try the closest endpoint pairs first; for each, a straight run on any
            # layer, then L-shaped runs on F.Cu that bend around blocking pads.
            pairs = sorted(((sx, sy, tx, ty) for sx, sy in src for tx, ty in main_pts),
                           key=lambda q: (q[0] - q[2]) ** 2 + (q[1] - q[3]) ** 2)
            def route_on(layer, segs, sx, sy, tx, ty):
                """Place a (possibly bent) run on one layer, with end vias if inner."""
                if not all(obst[layer].clear(ax, ay, bx, by, code) for ax, ay, bx, by in segs):
                    return False
                if layer != f_cu and not all(
                        o.point_clear(sx, sy, code) and o.point_clear(tx, ty, code)
                        for o in obst.values()):
                    return False
                if layer != f_cu:
                    add_via(board, sx, sy, code, f_cu, b_cu)
                    add_via(board, tx, ty, code, f_cu, b_cu)
                for ax, ay, bx, by in segs:
                    add_track(board, ax, ay, bx, by, layer, code)
                return True

            routed = False
            for sx, sy, tx, ty in pairs[:500]:
                dist = ((sx - tx) ** 2 + (sy - ty) ** 2) ** 0.5
                if dist <= MERGE:
                    add_track(board, sx, sy, tx, ty, f_cu, code)
                    routed = True
                    break
                # straight, then L-shaped (two elbows), on each layer
                shapes = [[(sx, sy, tx, ty)],
                          [(sx, sy, sx, ty), (sx, ty, tx, ty)],
                          [(sx, sy, tx, sy), (tx, sy, tx, ty)]]
                for layer in cu:
                    if any(route_on(layer, segs, sx, sy, tx, ty) for segs in shapes):
                        routed = True
                        break
                if routed:
                    break

            # Boxed-pad escape: F.Cu stub from the pad to a nearby spot clear on ALL
            # layers (the via is wider than a track, so every layer is point-checked),
            # via down, B.Cu run to a clear point by a target, via back up.
            if not routed:
                allobst = list(obst.values())
                step = pcbnew.FromMM(0.2)
                for sx, sy in src:
                    for k in range(2, 16):
                        for ox, oy in ((k, 0), (-k, 0), (0, k), (0, -k), (k, k), (-k, k), (k, -k), (-k, -k)):
                            vx, vy = sx + ox * step, sy + oy * step
                            if not (obstF.clear(sx, sy, vx, vy, code)
                                    and all(o.point_clear(vx, vy, code) for o in allobst)):
                                continue
                            for tx, ty in sorted(main_pts, key=lambda p: (p[0] - vx) ** 2 + (p[1] - vy) ** 2)[:10]:
                                if not all(o.point_clear(tx, ty, code) for o in allobst):
                                    continue
                                # route the buried hop on whichever inner/bottom layer is clear
                                rl = next((L for L in cu if L != f_cu and obst[L].clear(vx, vy, tx, ty, code)), None)
                                if rl is None:
                                    continue
                                add_track(board, sx, sy, vx, vy, f_cu, code)
                                add_via(board, vx, vy, code, f_cu, b_cu)
                                add_track(board, vx, vy, tx, ty, rl, code)
                                add_via(board, tx, ty, code, f_cu, b_cu)
                                routed = True
                                break
                            if routed:
                                break
                        if routed:
                            break
                    if routed:
                        break
            if routed:
                added += 1
            else:
                print(f"  UNRESOLVED: {name}")
            main_pts.extend(src)

    pcbnew.SaveBoard(path, board)
    print(f"finish_route: added {added} connections")


if __name__ == "__main__":
    main()
