"""Render a component placement diagram from the KiCad pick-and-place (CPL) file.

Reads the assembly CPL CSV (designator, value, package, X, Y, rotation, layer)
and draws each part to scale on the board outline, coloured by component class.
This gives a faithful "where is every component" view without needing the
original EDA source files, which are not present in this repository.
"""

from __future__ import annotations

import csv
import re
from dataclasses import dataclass
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.patches as mpatches
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle
from matplotlib.transforms import Affine2D

HERE = Path(__file__).resolve().parent
PROD = HERE.parent / "Production Files"
CPL_FILE = PROD / "Pick and Place" / "MESHTASTIC NODE-CPL.csv"
OUT_FILE = HERE.parent / "diagrams" / "component-placement.png"

# Board profile size from the Gerber job file (GeneralSpecs.Size), in mm.
BOARD_W = 30.1
BOARD_H = 40.1

# Approximate body footprint sizes (width, height) in mm, keyed by a substring
# match against the package name. These are body sizes for drawing only.
FOOTPRINT_DIMS: list[tuple[str, tuple[float, float]]] = [
    ("QFN-56", (7.0, 7.0)),
    ("SOIC-8", (5.3, 5.3)),
    ("WSON-6", (2.0, 2.0)),
    ("SOT-563", (1.6, 1.2)),
    ("Crystal_SMD_3225", (3.2, 2.5)),
    ("Wio-SX1262", (16.0, 18.0)),
    ("GT-USB-8016B", (9.0, 7.3)),
    ("U.FL", (2.6, 2.6)),
    ("SW_SPST_B3U", (3.5, 2.9)),
    ("LED_0603", (1.6, 0.8)),
    ("L_0603", (1.6, 0.8)),
    ("C_0603", (1.6, 0.8)),
    ("R_0603", (1.6, 0.8)),
    ("0603", (1.6, 0.8)),
]

# Component class -> (colour, human label). Class is inferred from designator.
CLASS_STYLE: dict[str, tuple[str, str]] = {
    "IC": ("#d62728", "IC / active"),
    "MODULE": ("#9467bd", "LoRa module"),
    "CONN": ("#1f77b4", "Connector"),
    "SW": ("#ff7f0e", "Switch"),
    "LED": ("#2ca02c", "LED"),
    "XTAL": ("#8c564b", "Crystal"),
    "L": ("#17becf", "Inductor"),
    "C": ("#7f7f7f", "Capacitor"),
    "R": ("#bcbd22", "Resistor"),
}

# Friendly function notes for the headline parts, shown on the diagram.
PART_NOTES: dict[str, str] = {
    "U3": "ESP32-S3R8 MCU",
    "U5": "WIO-SX1262 LoRa",
    "U4": "GD25Q64 flash",
    "U2": "TLV75901 LDO",
    "U1": "USBLC6 ESD",
    "J1": "USB-C",
    "AE1": "U.FL ant.",
    "Y1": "40MHz xtal",
}


@dataclass
class Part:
    ref: str
    value: str
    package: str
    x: float
    y: float
    rot: float
    layer: str


def classify(ref: str) -> str:
    prefix = re.match(r"[A-Za-z]+", ref)
    p = prefix.group(0).upper() if prefix else ""
    if p == "U" and ref.upper() == "U5":
        return "MODULE"
    if p == "U":
        return "IC"
    if p in {"J", "AE"}:
        return "CONN"
    if p == "SW":
        return "SW"
    if p == "D":
        return "LED"
    if p == "Y":
        return "XTAL"
    if p in {"L", "C", "R"}:
        return p
    return "C"


def dims_for(package: str) -> tuple[float, float]:
    for key, wh in FOOTPRINT_DIMS:
        if key.lower() in package.lower():
            return wh
    return (1.6, 0.8)


def load_parts(path: Path) -> list[Part]:
    parts: list[Part] = []
    with path.open(newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            parts.append(
                Part(
                    ref=row["Designator"].strip('"'),
                    value=row["Val"].strip('"'),
                    package=row["Package"].strip('"'),
                    x=float(row["Mid X"]),
                    y=float(row["Mid Y"]),
                    rot=float(row["Rotation"]),
                    layer=row["Layer"].strip('"'),
                )
            )
    return parts


def draw(parts: list[Part], out: Path) -> None:
    fig, ax = plt.subplots(figsize=(9, 11))

    # Board outline.
    ax.add_patch(
        Rectangle(
            (0, 0), BOARD_W, BOARD_H,
            facecolor="#114b1f", edgecolor="#0a2e13", linewidth=2, zorder=0,
        )
    )

    used_classes: set[str] = set()
    for part in parts:
        cls = classify(part.ref)
        used_classes.add(cls)
        colour, _ = CLASS_STYLE[cls]
        w, h = dims_for(part.package)

        tf = (
            Affine2D().rotate_deg(part.rot).translate(part.x, part.y) + ax.transData
        )
        ax.add_patch(
            Rectangle(
                (-w / 2, -h / 2), w, h,
                facecolor=colour, edgecolor="white", linewidth=0.5,
                alpha=0.92, transform=tf, zorder=2,
            )
        )

        note = PART_NOTES.get(part.ref)
        big = max(w, h) >= 3.0
        label = part.ref + (f"\n{note}" if (note and big) else "")
        ax.text(
            part.x, part.y, label,
            ha="center", va="center",
            fontsize=6 if big else 4.2,
            color="white", weight="bold", zorder=3,
        )

    ax.set_xlim(-2, BOARD_W + 2)
    ax.set_ylim(-2, BOARD_H + 2)
    ax.set_aspect("equal")
    ax.set_xlabel("X (mm)")
    ax.set_ylabel("Y (mm)")
    ax.set_title(
        f"MESHTASTIC NODE - component placement (top)\n"
        f"{BOARD_W} x {BOARD_H} mm, 4 layers, {len(parts)} placed parts",
        fontsize=11,
    )
    ax.grid(True, linestyle=":", alpha=0.3)

    handles = [
        mpatches.Patch(color=CLASS_STYLE[c][0], label=CLASS_STYLE[c][1])
        for c in CLASS_STYLE
        if c in used_classes
    ]
    ax.legend(handles=handles, loc="upper left", bbox_to_anchor=(1.01, 1.0), fontsize=8)

    out.parent.mkdir(parents=True, exist_ok=True)
    fig.tight_layout()
    fig.savefig(out, dpi=200, bbox_inches="tight")
    print(f"wrote {out}")


def main() -> None:
    parts = load_parts(CPL_FILE)
    draw(parts, OUT_FILE)


if __name__ == "__main__":
    main()
