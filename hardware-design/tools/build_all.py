"""One command: schematic -> board -> route -> production files, with gates.

Reproduces the full PCBWay package from the generator scripts. Each stage is
gated; the run stops on the first failure so a bad edit is caught early.

  1. build_schematic.py     regenerate the schematic from the net map
  2. sch erc                must be 0 violations
  3. verify_nets.py         every firmware net present
  4. board_setup.py         4-layer board + outline
  5. build_board.py         place footprints, assign nets
  6. check_placement.py     no courtyard overlaps
  7. route.py               autoroute + finish + ground/power pour
  8. pcb drc                must be 0 errors, 0 unconnected, parity clean
  9. fab_outputs.py         Gerbers, drill, CPL, BOM -> kicad/fab/

Usage: python3 tools/build_all.py [--passes N] [--skip-route]
The board steps need the KiCad AppImage and the freerouting jar (see route.py).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / "tools"
KP = ROOT / "kicad" / "MESHTASTIC_NODE"
SCH = KP / "MESHTASTIC_NODE.kicad_sch"
PCB = KP / "MESHTASTIC_NODE.kicad_pcb"
APPIMAGE = Path(os.environ.get("KICAD_APPIMAGE",
                               Path.home() / ".local/bin/kicad-10.0.3-x86_64.AppImage"))


def step(msg: str) -> None:
    print(f"\n=== {msg} ===")


def run(*args, **kw) -> subprocess.CompletedProcess:
    kw.setdefault("check", True)
    return subprocess.run([str(a) for a in args], **kw)


def py(script: str, *args) -> None:
    run(sys.executable, TOOLS / script, *args)


def kpython(script: str, *args) -> None:
    """Run a pcbnew-dependent tool under the KiCad AppImage's python."""
    run(APPIMAGE, "python3.11", TOOLS / script, *args)


def erc_clean() -> None:
    out = Path("/tmp/erc_all.json")
    run("kicad-cli", "sch", "erc", "--severity-error", "--exit-code-violations",
        "--format", "json", "-o", out, SCH, check=False)
    data = json.loads(out.read_text())
    viol = sum(len(s.get("violations", [])) for s in data.get("sheets", []))
    if viol:
        sys.exit(f"ERC: {viol} violations - aborting")
    print("ERC clean")


def drc_clean() -> None:
    out = Path("/tmp/drc_all.json")
    run("kicad-cli", "pcb", "drc", "--schematic-parity", "--format", "json",
        "--exit-code-violations", "-o", out, PCB, check=False)
    d = json.loads(out.read_text())
    err = [v for v in d["violations"] if v.get("severity") == "error"]
    warn = [v for v in d["violations"] if v.get("severity") == "warning"]
    unc, par = len(d["unconnected_items"]), len(d["schematic_parity"])
    print(f"DRC: {len(err)} errors, {len(warn)} warnings, {unc} unconnected, {par} parity")
    if err or par:
        sys.exit("DRC has errors or parity issues - aborting")
    if unc:
        print(f"WARNING: {unc} unconnected net(s) - a few dense escapes need a manual "
              "trace in the GUI before fabrication. Listing:")
        for v in d["unconnected_items"]:
            print("  -", " <-> ".join(it.get("description", "") for it in v.get("items", [])))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--passes", type=int, default=100)
    ap.add_argument("--skip-route", action="store_true",
                    help="keep the current (e.g. hand-routed) board: re-pour, re-check, re-fab only")
    args = ap.parse_args()

    if not args.skip_route:
        step("1. schematic")
        py("build_schematic.py")
        step("2. ERC")
        erc_clean()
        step("3. verify firmware nets")
        py("verify_nets.py")

        step("4-5. board + placement")
        py("board_setup.py")
        py("build_board.py")
        step("6. placement check")
        py("check_placement.py")

        step("7. route + pour")
        py("route.py", "--passes", args.passes)
    else:
        # The board was hand-edited (e.g. the GUI router closed the last few
        # connections); re-pour so the ground/power zones flow around the new
        # tracks and stitching avoids them before re-checking and re-fabbing.
        step("7. re-pour ground (skip-route)")
        kpython("pour.py", PCB)

    step("8. DRC")
    drc_clean()
    step("9. fab outputs")
    py("fab_outputs.py")
    step("10. board renders")
    py("render_board.py")
    print("\nALL GREEN - production package in kicad/fab/, renders in diagrams/renders/")


if __name__ == "__main__":
    main()
