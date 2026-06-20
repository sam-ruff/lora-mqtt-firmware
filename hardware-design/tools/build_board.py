"""Populate the board from the schematic netlist and the pick-and-place file.

Exports the netlist (source of truth), then for every component loads its
footprint, places it at the manufactured X/Y/rotation/side from the CPL, and
assigns each pad's net. The result is a placed, net-correct board with a full
ratsnest, ready to route. Run after tools/board_setup.py.
"""

from __future__ import annotations

import csv
import re
import subprocess
import uuid
from pathlib import Path

from kiutils.board import Board
from kiutils.footprint import Footprint, Model
from kiutils.items.common import Coordinate, Net, Position

import placement

# Footprints whose 3D model is not in the bundled KiCad library: attach the
# vendored model from kicad/libs/3dmodels/ so they appear in renders. Three are
# real models pulled from kicad-packages3D; the Wio module is a box stand-in
# (tools/make_box_models.py).
LOCAL_MODELS = {
    "Wio-SX1262",
    "USB_C_Plug_ShenzhenJingTuoJin_918-118A2021Y40002_Vertical",
    "U.FL_Molex_MCRF_73412-0110_Vertical",
    "SOIC-8_5.23x5.23mm_P1.27mm",
}

ROOT = Path(__file__).resolve().parent.parent
KP = ROOT / "kicad" / "MESHTASTIC_NODE"
PCB = KP / "MESHTASTIC_NODE.kicad_pcb"
SCH = KP / "MESHTASTIC_NODE.kicad_sch"
FPLIB = ROOT / "kicad" / "libs" / "kicad-footprints"
WIO_PRETTY = ROOT / "kicad" / "libs" / "wio.pretty"
CPL = ROOT / "Production Files" / "Pick and Place" / "MESHTASTIC NODE-CPL.csv"
NET = Path("/tmp/node.net")


def export_netlist() -> str:
    subprocess.run(["kicad-cli", "sch", "export", "netlist", "-o", str(NET), str(SCH)], check=True)
    return NET.read_text()


def parse_netlist(text: str):
    comps: dict[str, tuple[str, str, dict[str, str]]] = {}
    for chunk in re.split(r"\(comp\b", text)[1:]:
        ref = re.search(r'\(ref\s+"([^"]+)"\)', chunk)
        fp = re.search(r'\(footprint\s+"([^"]+)"\)', chunk)
        val = re.search(r'\(value\s+"([^"]*)"\)', chunk)
        # Capture the symbol fields (LCSC etc.) so the footprint can carry the
        # same fields - otherwise schematic parity flags a field mismatch.
        fields = {
            m.group(1): m.group(2)
            for m in re.finditer(r'\(field\s+\(name\s+"([^"]+)"\)\s+"([^"]*)"\)', chunk)
        }
        if ref and fp:
            comps[ref.group(1)] = (fp.group(1), val.group(1) if val else "", fields)
    pin_net: dict[str, dict[str, str]] = {}
    for chunk in re.split(r"\(net\b", text)[1:]:
        nm = re.search(r'\(name\s+"([^"]*)"', chunk)
        if not nm:
            continue
        for nd in re.finditer(r'\(node\s+\(ref\s+"([^"]+)"\)\s*\(pin\s+"([^"]+)"\)', chunk):
            pin_net.setdefault(nd.group(1), {})[nd.group(2)] = nm.group(1)
    return comps, pin_net


def load_cpl() -> dict[str, tuple[float, float, float, str]]:
    out = {}
    with CPL.open(newline="") as fh:
        for row in csv.DictReader(fh):
            out[row["Designator"].strip('"')] = (
                float(row["Mid X"]), float(row["Mid Y"]),
                float(row["Rotation"]), row["Layer"].strip('"'))
    return out


def load_footprint(fpid: str) -> Footprint:
    lib, name = fpid.split(":", 1)
    if lib == "Wio-SX1262":
        return Footprint.from_file(str(WIO_PRETTY / f"{name}.kicad_mod"))
    return Footprint.from_file(str(FPLIB / f"{lib}.pretty" / f"{name}.kicad_mod"))


def bake_pad_rotation(fp: Footprint, rot: float) -> None:
    """Add the footprint rotation to each pad's own angle.

    KiCad rotates pad *positions* with the footprint but treats each pad's stored
    angle as absolute, so a rotated SMD pad keeps its unrotated shape and fine-pitch
    pads overlap into a bar (phantom shorts). pcbnew bakes the footprint angle into
    every pad; kiutils does not, so we replicate it here.
    """
    if not rot:
        return
    for pad in fp.pads:
        base = pad.position.angle or 0
        pad.position = Position(pad.position.X, pad.position.Y, (base + rot) % 360)


def fresh_uuids(fp: Footprint) -> None:
    fp.tstamp = str(uuid.uuid4())
    for p in fp.pads:
        p.tstamp = str(uuid.uuid4())
    for g in fp.graphicItems:
        if hasattr(g, "tstamp"):
            g.tstamp = str(uuid.uuid4())


def set_ref_val(fp: Footprint, ref: str, val: str) -> None:
    if isinstance(fp.properties, dict):
        fp.properties["Reference"] = ref
        fp.properties["Value"] = val
    for g in fp.graphicItems:
        if type(g).__name__ == "FpText":
            if g.type == "reference":
                g.text = ref
                # Move designators to the fab layer: at this density they would
                # overlap pads/each other on silk. Assembly uses the CPL + BOM.
                g.layer = "F.Fab" if fp.layer == "F.Cu" else "B.Fab"
            elif g.type == "value":
                g.text = val
                g.hide = True


def main() -> None:
    comps, pin_net = parse_netlist(export_netlist())
    cpl = load_cpl()
    board = Board.from_file(str(PCB))

    # Idempotent: drop any prior placement/routing so re-running just re-places
    # (keeps the Edge.Cuts outline in graphicItems). Routing is a later step.
    board.footprints = []
    board.traceItems = []
    board.zones = []

    names = sorted({n for m in pin_net.values() for n in m.values()})
    code = {n: i + 1 for i, n in enumerate(names)}
    board.nets = [Net(number=0, name="")] + [Net(number=code[n], name=n) for n in names]

    placed, missing, no_cpl = 0, [], []
    for ref, (fpid, val, fields) in comps.items():
        if ref.startswith("#"):
            continue
        try:
            fp = load_footprint(fpid)
        except Exception as e:  # noqa: BLE001 - report and continue
            missing.append(f"{ref}({fpid}): {type(e).__name__}")
            continue
        fresh_uuids(fp)
        # The footprint's library id must equal the symbol's Footprint field, or
        # schematic parity reports a footprint/symbol mismatch.
        fp.libraryNickname, fp.entryName = fpid.split(":", 1)
        if fp.entryName in LOCAL_MODELS:
            fp.models = [Model(
                path=f"${{KIPRJMOD}}/../libs/3dmodels/{fp.entryName}.wrl",
                pos=Coordinate(0, 0, 0), scale=Coordinate(1, 1, 1),
                rotate=Coordinate(0, 0, 0))]
        set_ref_val(fp, ref, val)
        # Carry the symbol fields onto the footprint so schematic parity sees the
        # same Datasheet/Description/LCSC on both sides.
        if isinstance(fp.properties, dict):
            for key in ("Datasheet", "Description"):
                fp.properties[key] = fields.get(key, "")
            lcsc = fields.get("LCSC")
            if lcsc:
                fp.properties["LCSC"] = lcsc
        if ref in placement.OVERRIDES:
            x, y, rot = placement.OVERRIDES[ref]
            fp.position = Position(x, y, rot)
            fp.layer = "F.Cu"
            bake_pad_rotation(fp, rot)
        elif ref in cpl:
            x, y, rot, layer = cpl[ref]
            fp.position = Position(x, y, rot)
            fp.layer = "F.Cu" if layer.lower() == "top" else "B.Cu"
            bake_pad_rotation(fp, rot)
        else:
            no_cpl.append(ref)
        for pad in fp.pads:
            net = pin_net.get(ref, {}).get(pad.number)
            if net:
                pad.net = Net(number=code[net], name=net)
        board.footprints.append(fp)
        placed += 1

    board.to_file(str(PCB))
    print(f"placed {placed} footprints, {len(board.nets) - 1} nets")
    if no_cpl:
        print("no CPL position for:", ", ".join(no_cpl))
    if missing:
        print("MISSING footprints:", "; ".join(missing))


if __name__ == "__main__":
    main()
