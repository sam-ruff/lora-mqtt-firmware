"""Check captured schematic nets against the firmware pin map (ground truth).

Exports the netlist with kicad-cli and asserts that the connections defined in the
firmware (src/main.rs) are present. Run after schematic capture. If you name nets
differently during capture, update EXPECTED below (matching is by substring, so a
label like /LORA_NSS still matches the key LORA_NSS).
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCH = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_sch"
NET = Path("/tmp/meshtastic_node.net")

# Net name (substring) -> reference designators that must share that net.
# Derived from src/main.rs: SCLK=GPIO7, MISO=GPIO8, MOSI=GPIO9, NSS=GPIO41,
# DIO1=GPIO39, NRST=GPIO42, BUSY=GPIO40; USB D+=GPIO20, D-=GPIO19; LED=GPIO48.
EXPECTED: dict[str, set[str]] = {
    "3V3": {"U2", "U3", "U4", "U5"},
    "LORA_NSS": {"U3", "U5"},
    "LORA_SCK": {"U3", "U5"},
    "LORA_MOSI": {"U3", "U5"},
    "LORA_MISO": {"U3", "U5"},
    "LORA_BUSY": {"U3", "U5"},
    "LORA_DIO1": {"U3", "U5"},
    "LORA_NRST": {"U3", "U5"},
    "USB_DP": {"J1", "U1", "U3"},
    "USB_DM": {"J1", "U1", "U3"},
    "LED_GPIO": {"U3", "R6"},   # GPIO48 -> series resistor
    "LED_R": {"R6", "D1"},      # resistor -> LED
}


def export_netlist() -> str:
    subprocess.run(
        ["kicad-cli", "sch", "export", "netlist", "--output", str(NET), str(SCH)],
        check=True,
    )
    return NET.read_text()


def parse_nets(text: str) -> dict[str, set[str]]:
    nets: dict[str, set[str]] = {}
    for chunk in re.split(r"\(net\b", text)[1:]:
        name_m = re.search(r'\(name\s+"?([^"\)\s]+)"?\)', chunk)
        if not name_m:
            continue
        refs = set(re.findall(r'\(ref\s+"?([^"\s\)]+)"?\)', chunk))
        nets[name_m.group(1)] = refs
    return nets


def main() -> int:
    text = export_netlist()
    nets = parse_nets(text)
    ok = True
    for key, want in EXPECTED.items():
        match = next((r for n, r in nets.items() if key.lower() in n.lower()), None)
        if match is None:
            print(f"FAIL {key}: no net found")
            ok = False
            continue
        missing = want - match
        if missing:
            print(f"FAIL {key}: missing {sorted(missing)} (has {sorted(match)})")
            ok = False
        else:
            print(f"ok   {key}: {sorted(match & want)}")
    print("all firmware nets present" if ok else "net verification FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
