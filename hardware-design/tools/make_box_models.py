"""Generate simple box 3D models for parts the KiCad model library does not cover.

The bundled KiCad 3D models miss a few of this board's footprints (the substitute
USB-C, the U.FL, the 5.23 mm SOIC, and the custom Wio module). Without a model
those parts vanish from `kicad-cli pcb render`. This writes a sized, coloured box
.wrl for each into kicad/libs/3dmodels/; `build_board.py` attaches them. Boxes are
schematic stand-ins, not mechanically exact.

VRML note: KiCad scales .wrl geometry by 2.54 (1 unit = 0.1 inch), so dimensions
here are divided by 2.54, and each box is lifted by half its height to sit on the
board.

Usage: python3 tools/make_box_models.py
"""

from __future__ import annotations

from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "kicad" / "libs" / "3dmodels"
SCALE = 2.54

# name -> (width, depth, height in mm, (r, g, b)). Only the custom Wio module
# needs a stand-in; the USB-C plug, U.FL and SOIC use real vendored models.
BOXES = {
    "Wio-SX1262": (11.0, 11.6, 2.0, (0.11, 0.13, 0.16)),
}

TEMPLATE = """#VRML V2.0 utf8
# generated stand-in box model
Transform {{
  translation 0 0 {zoff:.4f}
  children [
    Shape {{
      appearance Appearance {{ material Material {{
        diffuseColor {r:.2f} {g:.2f} {b:.2f}
        specularColor 0.3 0.3 0.3
      }} }}
      geometry Box {{ size {x:.4f} {y:.4f} {z:.4f} }}
    }}
  ]
}}
"""


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, (w, d, h, (r, g, b)) in BOXES.items():
        text = TEMPLATE.format(
            zoff=(h / 2) / SCALE, r=r, g=g, b=b,
            x=w / SCALE, y=d / SCALE, z=h / SCALE,
        )
        (OUT / f"{name}.wrl").write_text(text)
        print(f"wrote {name}.wrl ({w}x{d}x{h} mm)")


if __name__ == "__main__":
    main()
