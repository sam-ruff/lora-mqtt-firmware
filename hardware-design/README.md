# Hardware design

PCB design and tooling for the MESHTASTIC NODE board (ESP32-S3 + Wio-SX1262 LoRa).
There are two design states:

- `Production Files/` - the original as-built 4-layer manufacturing outputs. Send
  these straight to a fab if you just want the existing board made.
- `kicad/MESHTASTIC_NODE/` - an editable 4-layer KiCad project, generated entirely
  from scripts in `tools/` so a schematic edit reproduces a full production package.

Generated views live in `diagrams/`, component datasheets and a spec digest in
`datasheets/`, and helper scripts in `tools/`.

## Schematic-to-production pipeline

The whole flow is one command (needs the KiCad AppImage and the freerouting jar -
see "Routing toolchain" below):

```bash
python3 tools/build_all.py            # schematic -> board -> route -> kicad/fab/
```

It runs, and gates on, each stage: regenerate the schematic
(`build_schematic.py`), ERC, verify every firmware net (`verify_nets.py`), build
the 4-layer board (`board_setup.py` + `build_board.py`), check placement
(`check_placement.py`), autoroute + finish + pour ground/power planes
(`route.py`), DRC (must be 0 errors / 0 unconnected / parity-clean), then export
the Gerbers, drill, CPL and BOM (`fab_outputs.py`). Upload
`kicad/fab/gerbers.zip` plus `BOM.csv` and `CPL.csv` to PCBWay.

To change the board, edit the net map in `tools/build_schematic.py` (and the
placement overrides in `tools/placement.py` if a part needs moving), then re-run
`build_all.py`. Use `--skip-route` to re-check and re-export the current routed
board without re-routing.

### Routing toolchain

`tools/route.py` exports a Specctra DSN with the bundled `pcbnew`, autoroutes the
signals with freerouting (headless), imports the SES back, closes any stragglers
(`finish_route.py`), then pours the GND/+3V3 planes and stitching vias
(`pour.py`). Install freerouting once:

```bash
mkdir -p ~/.local/share/freerouting
curl -L -o ~/.local/share/freerouting/freerouting-2.2.4.jar \
  https://github.com/freerouting/freerouting/releases/download/v2.2.4/freerouting-2.2.4.jar
```

Override the tool paths with the `KICAD_APPIMAGE` and `FREEROUTING_JAR` env vars.

## Tools to install first

Commands assume Ubuntu Linux. `uv` and Node.js are expected to be present already.

### KiCad 9 or newer (required)

Provides `kicad-cli`, the GUI editors, and the bundled ngspice/3D tools. The
design was authored in 9.0.1, and the headless DRC/ERC checks need v9+; KiCad 7
(the default in some Ubuntu repos) cannot open v9 files or run the CLI checks.

```bash
# option 1 - PPA (needs sudo)
sudo add-apt-repository ppa:kicad/kicad-9.0-releases
sudo apt update && sudo apt install --install-recommends kicad

# option 2 - Flatpak (no sudo)
flatpak install --user flathub org.kicad.KiCad
flatpak run --command=kicad-cli org.kicad.KiCad version

# option 3 - AppImage (no sudo) - the method in use here
# download the KiCad AppImage, put it on a writeable PATH dir, and add a wrapper
# that dispatches the bundled CLI (the AppImage selects its tool from the first arg):
mv kicad-*-x86_64.AppImage ~/.local/bin/ && chmod +x ~/.local/bin/kicad-*.AppImage
printf '#!/bin/sh\nexec "$HOME/.local/bin/kicad-10.0.3-x86_64.AppImage" kicad-cli "$@"\n' > ~/.local/bin/kicad-cli
printf '#!/bin/sh\nexec "$HOME/.local/bin/kicad-10.0.3-x86_64.AppImage" kicad "$@"\n' > ~/.local/bin/kicad
chmod +x ~/.local/bin/kicad-cli ~/.local/bin/kicad
```

For headless 3D board renders also install `xvfb`.

### Diagram rendering

```bash
# block / component-overview diagrams (needs Node.js + Chrome/Chromium)
npm install -g @mermaid-js/mermaid-cli
# rendering points Puppeteer at the system Chrome via tools/puppeteer-config.json

# to-scale placement diagram
uv pip install matplotlib
```

### Gerber inspection (no KiCad source needed)

```bash
uv tool install gerbonara      # render Gerbers to SVG
uvx cairosvg in.svg -o out.png -s 3   # rasterise SVG to PNG so it can be viewed
```

## Regenerating diagrams

See the top-level `CLAUDE.md` for the regeneration commands and the readability
checklist that must pass before a diagram change is considered done.
