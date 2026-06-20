# Editable source recovery plan

> Status: this recovery is largely complete. The editable project exists at
> `kicad/MESHTASTIC_NODE/` - the schematic is captured and ERC-clean, and the
> board is populated (placed, not yet routed). The board has been rebuilt as
> **2-layer** (the original manufactured board under `Production Files/` is
> 4-layer). What follows is the original plan, kept for reference; the net map and
> open questions below still apply during layout.

The repo originally had only manufacturing outputs (Gerbers, CPL, BOM), no
editable KiCad source. This is the plan that rebuilt an editable KiCad project so
the design can be reviewed, changed and simulated. Schematic-first: never base the
design on a Gerber import (netless copper).

## Approach

Hybrid rebuild, not reverse engineering:

1. Reuse the Seeed Wio-SX1262 KiCad block for the radio (symbol, footprint,
   reference wiring).
2. Use standard KiCad libraries for everything else - the BOM footprint names are
   already KiCad library names.
3. Take the connectivity (nets) from the part datasheets, the standard ESP32-S3
   reference circuit, and the firmware pin map (ground truth).
4. Use the Gerbers only as a placement/outline underlay (board outline is
   30 x 40 mm). The editable board is 2-layer; the original Gerbers are 4-layer.
5. Validate every step with `kicad-cli sch erc` / `pcb drc`.

## Assets gathered (under `libs/`)

- `libs/seeed-opl/Seeed Studio Wio SX1262 for XIAO ESP32S3/` - Seeed's editable
  KiCad project for the module (symbol + footprint + reference wiring). Vendored
  (git history stripped).
- `libs/espressif/` - Espressif KiCad symbol library (`symbols/Espressif.kicad_sym`)
  and module footprints. Note: these are module footprints; the bare ESP32-S3R8
  uses the standard `Package_DFN_QFN:QFN-56-1EP_7x7mm_P0.4mm_EP4x4mm`.

## BOM -> KiCad symbol / footprint mapping

Footprints come straight from the BOM (all standard KiCad libs except the two
noted). Symbols are from standard KiCad libraries unless stated.

| Ref(s) | Part | Footprint (KiCad lib) | Symbol source |
| --- | --- | --- | --- |
| U3 | ESP32-S3R8 | Package_DFN_QFN:QFN-56-1EP_7x7mm_P0.4mm_EP4x4mm | RF_Module / Espressif lib |
| U5 | Wio-SX1262 | Seeed OPL footprint | Seeed OPL symbol |
| U4 | GD25Q64 | Package_SO:SOIC-8_5.3x5.3mm_P1.27mm | Memory_Flash (generic 25Qxx) |
| U2 | TLV75901 | Package_SON:WSON-6-1EP_2x2mm_P0.65mm_EP1x1.6mm | Regulator_Linear (adjustable) |
| U1 | USBLC6-2P6 | Package_TO_SOT_SMD:SOT-563 | Power_Protection:USBLC6-2P6 |
| J1 | GT-USB-8016B USB-C | custom (BOM: "PCB Footprints:GT-USB-8016B") - must be drawn or substituted | Connector USB_C_Receptacle |
| AE1 | U.FL | Connector_Coaxial:U.FL_Molex_MCRF_73412-0110_Vertical | Connector_Coaxial:U.FL |
| Y1 | 40MHz xtal | Crystal:Crystal_SMD_3225-4Pin_3.2x2.5mm | Device:Crystal_GND24 |
| D1 | LED | LED_SMD:LED_0603_1608Metric | Device:LED |
| SW1,SW2 | B3U-1000P | Button_Switch_SMD:SW_SPST_B3U-1000P | Switch:SW_Push |
| L1,L2 | inductors | Inductor_SMD:L_0603_1608Metric | Device:L |
| C1..C21 | caps | Capacitor_SMD:C_0603_1608Metric | Device:C |
| R1..R7 | resistors | Resistor_SMD:R_0603_1608Metric | Device:R |

Two footprints are not standard and need attention: J1 (custom USB-C land pattern
`GT-USB-8016B`) and U5 (use the Seeed OPL footprint). The custom J1 footprint must
be drawn from the connector datasheet or matched to an equivalent 16-pin USB-C
receptacle footprint.

## Net map (design intent)

Firmware-confirmed nets are marked [fw] (from `src/main.rs`). Everything else is
inferred from datasheets and the standard ESP32-S3 reference design - mark each as
"verify" until checked against the copper.

Power
- VBUS(5V): J1 VBUS -> U2 IN; bulk C1/C2 (10uF). 
- U2 TLV75901: IN=5V, OUT=3V3, FB = divider node R2 (100K, OUT->FB) / R3 (20K,
  FB->GND) => 0.55 x (1 + 100/20) = 3.3V. Output decoupling C18 (1uF) / C19 (100nF).
  EN pin: verify (tied to IN/pullup vs GPIO control).
- 3V3 rail -> U3 (all VDD/VDD3P3), U4 VCC, U5 VCC.

USB
- J1 D+ = GPIO20 [fw], D- = GPIO19 [fw], via U1 USBLC6 (ESD in line, VBUS pin to
  5V).
- J1 CC1/CC2 -> R1 (5.1K) pulldown (UFP/sink). verify both CC lines.

MCU core (U3 ESP32-S3R8)
- Y1 40MHz on XTAL_P/XTAL_N with C20/C21 (22pF) load caps to GND.
- EN: pullup (10K) + SW2 to GND (reset); EN RC cap (C6 10nF?). verify.
- IO0 (BOOT): pullup (10K) + SW1 to GND (download boot). verify.
- Decoupling: 100nF per power pin (C3,C5,C7,C8,C9,C10,C13,C14); 2.2uF
  (C4,C11,C12) bulk near core/PSRAM.
- Strapping pins IO0/IO45/IO46/IO3: confirm default levels.

Flash (U4 GD25Q64)
- On the ESP32-S3 SPI0 flash bus (SCLK/CS/SI/SO/WP/HOLD dedicated pins); 3V3 +
  100nF decoupling. verify exact pin group.

Radio (U5 Wio-SX1262) - all [fw]
- NSS = GPIO41, SCK = GPIO7, MOSI = GPIO9, MISO = GPIO8, BUSY = GPIO40,
  DIO1 = GPIO39, NRESET = GPIO42, VCC = 3V3, GND.
- RF: module has integrated matching + TCXO + onboard IPEX. Board also has AE1
  (U.FL) and L1/L2/C15 - resolve whether the antenna comes off the module's IPEX
  or off AE1 via L1/L2/C15; remove any unused stub. OPEN QUESTION.

User IO
- D1 LED + R6 (1.5K) on GPIO48 [fw].
- SW1 -> IO0 (boot), SW2 -> EN (reset).

Open questions to resolve against the Gerbers/datasheets: U2 EN wiring; CC resistor
count; EN/BOOT RC values; exact flash pin group; the AE1 vs module-IPEX RF path;
roles of R4/R5/R7 (10K) and C6 (10nF).

## Build steps

1. Create the KiCad project (`.kicad_pro/.kicad_sch/.kicad_pcb`) and register the
   Seeed + Espressif libs plus the bundled standard libs.
2. Draw the schematic block by block (power -> USB -> MCU core -> flash -> radio ->
   UI), running `kicad-cli sch erc` after each block.
3. Set net classes / design rules from the fab limits (job file: ~0.13mm
   track-to-track, ~0.157mm min width) and the stackup from the gbrjob.
4. Assign footprints (table above), import to the board, place per the CPL, set the
   outline from Edge_Cuts.
5. Route (or import placement as a guide), pour ground, then `kicad-cli pcb drc
   --schematic-parity` until clean.
6. Apply the best-practices checklist in the top-level CLAUDE.md.

Keep each step a small git commit so a bad edit is one `git checkout` away.
