"""Pour the power/ground planes and stitching, then fill all zones.

Runs under the KiCad AppImage's python (needs pcbnew). The 4-layer stack is
F.Cu (signal) / In1.Cu (GND plane) / In2.Cu (+3V3 plane) / B.Cu (signal), matching
the original board. GND is also flooded on F.Cu and B.Cu in the signal gaps. KiCad
clips every zone to the Edge.Cuts outline.

Two modes:
  --planes   pour only the inner GND/+3V3 planes (run before routing so the
             Specctra DSN marks them as planes and the autorouter vias power and
             ground pins straight down to them).
  (default)  full pour: inner planes + F.Cu/B.Cu ground fill + ground stitching
             vias, tying the planes together. Run after routing.

Idempotent. On a 2-layer board it falls back to F.Cu/B.Cu ground only.

Usage: <appimage> python3.11 tools/pour.py board.kicad_pcb [--planes]
"""

import sys

import pcbnew

VIA_DIA, VIA_DRILL = 0.6, 0.3
CLEAR = 0.13
GRID = 1.5
BOARD_W, BOARD_H = 30.0, 40.0


def seg_pt(ax, ay, bx, by, px, py) -> float:
    """Distance from point (px,py) to segment (ax,ay)-(bx,by)."""
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return ((px - ax) ** 2 + (py - ay) ** 2) ** 0.5
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return ((px - ax - t * dx) ** 2 + (py - ay - t * dy) ** 2) ** 0.5


def main() -> None:
    path = sys.argv[1]
    planes_only = "--planes" in sys.argv[2:]
    board = pcbnew.LoadBoard(path)
    gnd = board.FindNet("GND").GetNetCode()
    pwr_net = board.FindNet("+3V3")
    pwr = pwr_net.GetNetCode() if pwr_net else None
    four = board.GetCopperLayerCount() >= 4
    f_cu, b_cu = board.GetLayerID("F.Cu"), board.GetLayerID("B.Cu")
    in1, in2 = board.GetLayerID("In1.Cu"), board.GetLayerID("In2.Cu")

    # Move reference designators to the fab layer and hide values: at this density
    # they overlap pads/each other on silk. Assembly is driven by the CPL + BOM.
    f_fab, b_fab = board.GetLayerID("F.Fab"), board.GetLayerID("B.Fab")
    for fp in list(board.GetFootprints()):
        fp.Reference().SetLayer(f_fab if fp.GetLayer() == f_cu else b_fab)
        fp.Value().SetVisible(False)

    # (layer, net) for every pour. The 4-layer board uses In2 as a +3V3 plane and
    # floods ground on F.Cu / In1.Cu / B.Cu (heavily stitched). In --planes mode
    # (pre-routing) only the In2 +3V3 plane is poured, so the DSN marks it a plane
    # and the autorouter vias the 3V3 pins down to it.
    if four:
        plan = [(in1, gnd), (in2, gnd), (f_cu, gnd), (b_cu, gnd)]
        gnd_layers = [f_cu, in1, in2, b_cu]
    else:
        plan = [(f_cu, gnd), (b_cu, gnd)]
        gnd_layers = [f_cu, b_cu]

    for z in list(board.Zones()):
        board.Remove(z)
    for t in list(board.GetTracks()):
        if (t.Type() == pcbnew.PCB_VIA_T and t.GetNetCode() == gnd
                and t.GetDrillValue() == pcbnew.FromMM(VIA_DRILL)):
            board.Remove(t)

    zones = {}
    for layer, net in plan:
        if net is None:
            continue
        z = pcbnew.ZONE(board)
        z.SetLayer(layer)
        z.SetNetCode(net)
        z.SetLocalClearance(pcbnew.FromMM(0.2))
        z.SetMinThickness(pcbnew.FromMM(0.2))
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_FULL)
        z.SetIslandRemovalMode(0)              # drop fill islands with no net connection
        z.SetIsFilled(False)
        o = z.Outline()
        o.NewOutline()
        for x, y in [(-1, -1), (31, -1), (31, 41), (-1, 41)]:
            o.Append(pcbnew.FromMM(x), pcbnew.FromMM(y))
        board.Add(z)
        zones[(layer, net)] = z

    pcbnew.ZONE_FILLER(board).Fill(board.Zones())

    n = 0
    if not planes_only:
        # Stitch the ground layers together where every ground fill is solid.
        margin = pcbnew.FromMM(VIA_DIA / 2 + CLEAR)
        ring = [(0, 0), (margin, 0), (-margin, 0), (0, margin), (0, -margin)]
        gnd_zones = [zones[(L, gnd)] for L in gnd_layers if (L, gnd) in zones]

        def add_via(px, py):
            v = pcbnew.PCB_VIA(board)
            v.SetPosition(pcbnew.VECTOR2I(int(px), int(py)))
            v.SetDrill(pcbnew.FromMM(VIA_DRILL))
            v.SetWidth(pcbnew.FromMM(VIA_DIA))
            v.SetNetCode(gnd)
            v.SetLayerPair(f_cu, b_cu)
            board.Add(v)

        def solid(zs, x, y) -> bool:
            return all(z.HitTestFilledArea(z.GetLayer(), pcbnew.VECTOR2I(int(x + dx), int(y + dy)), 0)
                       for z in zs for dx, dy in ring)

        y = GRID
        while y < BOARD_H:
            x = GRID
            while x < BOARD_W:
                px, py = pcbnew.FromMM(x), pcbnew.FromMM(y)
                if solid(gnd_zones, px, py):
                    add_via(px, py)
                    n += 1
                x += GRID
            y += GRID

        # Exposed/large ground pads (e.g. the ESP32 QFN paddle) need their own via
        # field into the planes; the coarse grid steps over them. The pad gives
        # F.Cu ground and B.Cu is a solid ground layer, so a through via grounds the
        # paddle as long as it clears any signal routed on the inner layers.
        b_zone = zones.get((b_cu, gnd))
        keep = pcbnew.FromMM(VIA_DIA / 2 + CLEAR)

        def inner_clear(layer, px, py) -> bool:
            for t in board.GetTracks():
                if t.GetNetCode() == gnd or not t.IsOnLayer(layer) or t.Type() == pcbnew.PCB_VIA_T:
                    continue
                a, e = t.GetStart(), t.GetEnd()
                if seg_pt(a.x, a.y, e.x, e.y, px, py) < keep + t.GetWidth() / 2:
                    return False
            for fp in board.GetFootprints():
                for pad in fp.Pads():
                    if pad.GetNetCode() != gnd and pad.IsOnLayer(layer):
                        d = ((pad.GetPosition().x - px) ** 2 + (pad.GetPosition().y - py) ** 2) ** 0.5
                        if d < keep + max(pad.GetSizeX(), pad.GetSizeY()) / 2:
                            return False
            return True

        fine = pcbnew.FromMM(0.8)
        for fp in list(board.GetFootprints()):
            for pad in fp.Pads():
                if pad.GetNetCode() != gnd or min(pad.GetSizeX(), pad.GetSizeY()) < pcbnew.FromMM(2):
                    continue
                c = pad.GetPosition()
                hx, hy = pad.GetSizeX() / 2 - margin, pad.GetSizeY() / 2 - margin
                gx = -hx
                while gx <= hx:
                    gy = -hy
                    while gy <= hy:
                        px, py = c.x + gx, c.y + gy
                        if (b_zone and b_zone.HitTestFilledArea(b_cu, pcbnew.VECTOR2I(int(px), int(py)), 0)
                                and inner_clear(in1, px, py) and inner_clear(in2, px, py)):
                            add_via(px, py)
                            n += 1
                        gy += fine
                    gx += fine
        pcbnew.ZONE_FILLER(board).Fill(board.Zones())

    pcbnew.SaveBoard(path, board)
    label = "planes" if planes_only else "full"
    print(f"pour ({label}): {len(zones)} zones, {n} stitching vias")


if __name__ == "__main__":
    main()
