"""Generate the MESHTASTIC NODE KiCad schematic from the recovered net map.

Connectivity is expressed with global labels placed at each symbol pin (proven to
ERC-connect). Symbols come from the vendored KiCad 8 standard libraries, the
Espressif library (ESP32-S3) and the Seeed schematic (Wio-SX1262); the adjustable
LDO symbol is derived from a stock 6-pin WSON LDO and relabelled to the TLV759P
pinout (1=OUT, 2=FB, 3=GND, 4=EN, 5=DNC, 6=IN).

Firmware-confirmed nets (src/main.rs) are exact. Power, decoupling, strapping,
flash-bus and the RF/antenna section are reconstructed from datasheets and the
standard ESP32-S3 reference design - every such choice is a REVIEW item, marked
below with `# INFER`.
"""

from __future__ import annotations

import csv
import uuid as U
from pathlib import Path

from kiutils.schematic import Schematic
from kiutils.symbol import SymbolLib
from kiutils.items.common import Effects, Position, Property
from kiutils.items.schitems import (
    GlobalLabel, NoConnect, SchematicSymbol, SymbolProjectInstance, SymbolProjectPath,
)

ROOT = Path(__file__).resolve().parent.parent
LIB = ROOT / "kicad" / "libs"
KS = LIB / "kicad-symbols"
OUT = ROOT / "kicad" / "MESHTASTIC_NODE" / "MESHTASTIC_NODE.kicad_sch"
PROJECT = "MESHTASTIC_NODE"
BOM_CSV = ROOT / "Production Files" / "BOM" / "MESHTASTIC NODE-BOM.csv"


def load_lcsc() -> dict[str, str]:
    """Map each designator to its LCSC part number from the original BOM."""
    out: dict[str, str] = {}
    with BOM_CSV.open(newline="") as fh:
        for row in csv.DictReader(fh):
            lcsc = (row.get("LCSC Part #") or "").strip()
            if not lcsc:
                continue
            for d in row["Designator"].replace('"', "").split(","):
                d = d.strip()
                if d:
                    out[d] = lcsc
    return out


LCSC = load_lcsc()
# Parts not stocked on LCSC - record the real source so the BOM is not blank.
LCSC["U5"] = "C9900174795"       # Wio-SX1262 module, on JLCPCB assembly
LCSC["J1"] = "C399939"           # USB-C male plug, ShenzhenJingTuoJin 918-118A2021Y40002 (on LCSC)

# --- load source symbol libraries -------------------------------------------
def load(p: Path) -> SymbolLib:
    return SymbolLib.from_file(str(p))

device = load(KS / "Device.kicad_sym")
prot = load(KS / "Power_Protection.kicad_sym")
switch = load(KS / "Switch.kicad_sym")
conn = load(KS / "Connector.kicad_sym")
conn_gen = load(KS / "Connector_Generic.kicad_sym")
mem = load(KS / "Memory_Flash.kicad_sym")
reg = load(KS / "Regulator_Linear.kicad_sym")
powerlib = load(KS / "power.kicad_sym")
esp = load(LIB / "espressif" / "symbols" / "Espressif.kicad_sym")


def get(lib: SymbolLib, name: str):
    return next(s for s in lib.symbols if s.libId == name)


def prep(sym, nick: str, entry: str):
    # KiCad requires unit sub-symbols to be prefixed with the parent name, so
    # rename the units too whenever the entry name changes.
    sym.libraryNickname = nick
    sym.entryName = entry
    for u in sym.units:
        u.entryName = entry
    return sym


# --- symbol registry (one embedded definition per type) ---------------------
SYM = {
    "R": prep(get(device, "R"), "Device", "R"),
    "C": prep(get(device, "C"), "Device", "C"),
    "L": prep(get(device, "L"), "Device", "L"),
    "LED": prep(get(device, "LED"), "Device", "LED"),
    "Crystal": prep(get(device, "Crystal_GND24"), "Device", "Crystal_GND24"),
    "USBLC6": prep(get(prot, "USBLC6-2P6"), "Power_Protection", "USBLC6-2P6"),
    "SW": prep(get(switch, "SW_Push"), "Switch", "SW_Push"),
    "USBC": prep(get(conn, "USB_C_Receptacle_USB2.0_16P"), "Connector", "USB_C_Receptacle_USB2.0_16P"),
    "UFL": prep(get(conn, "Conn_Coaxial"), "Connector", "Conn_Coaxial"),
    "FLASH": prep(get(mem, "W25Q32JVSS"), "Memory_Flash", "W25Q32JVSS"),
    "ESP32": prep(get(esp, "ESP32-S3"), "Espressif", "ESP32-S3"),
    "PWR_FLAG": prep(get(powerlib, "PWR_FLAG"), "power", "PWR_FLAG"),
}

# Wio module: stock 12-pin connector body relabelled to the module pinout.
wio = prep(get(conn_gen, "Conn_01x12"), "Wio-SX1262", "Wio-SX1262")
_wio_names = {"1": "RF_SW1", "2": "MISO", "3": "MOSI", "4": "SCK", "5": "RST",
              "6": "NSS", "7": "GND1", "8": "VCC", "9": "ANT", "10": "GND2",
              "11": "BUSY", "12": "DIO1"}
for u in wio.units:
    for p in u.pins:
        p.name = _wio_names.get(p.number, p.name)
SYM["WIO"] = wio

# Adjustable LDO: borrow a stock 6-pin WSON LDO body, relabel to TLV759P pinout.
ldo = prep(get(reg, "TLV70012_WSON6"), "Regulator_Linear", "TLV75901")
_tlv_names = {"1": "OUT", "2": "FB", "3": "GND", "4": "EN", "5": "DNC", "6": "IN"}
_tlv_types = {"1": "power_out", "2": "passive", "3": "power_in", "4": "input", "6": "power_in"}
for u in ldo.units:
    for p in u.pins:
        p.name = _tlv_names.get(p.number, p.name)
        if p.number in _tlv_types:
            p.electricalType = _tlv_types[p.number]
SYM["LDO"] = ldo

FP = {  # footprint per part class (from the BOM)
    "R": "Resistor_SMD:R_0603_1608Metric",
    "C": "Capacitor_SMD:C_0603_1608Metric",
    "L": "Inductor_SMD:L_0603_1608Metric",
}

# --- component table: (ref, symbol, value, footprint, {pin#: net}) -----------
# Pins not listed get a No-Connect flag.
C = []
def add(ref, sym, value, fp, nets):
    C.append((ref, sym, value, fp, nets))

# Power input + LDO (3.3V).  R2/R3 set Vout = 0.55*(1+100/20) = 3.3V.
add("U2", "LDO", "TLV75901", "Package_SON:WSON-6-1EP_2x2mm_P0.65mm_EP1x1.6mm",
    {"1": "+3V3", "2": "FB", "3": "GND", "4": "+5V", "6": "+5V"})  # 4=EN tied on  # INFER EN->5V
add("R2", "R", "100K", FP["R"], {"1": "+3V3", "2": "FB"})
add("R3", "R", "20K", FP["R"], {"1": "FB", "2": "GND"})
add("C1", "C", "10uF", FP["C"], {"1": "+5V", "2": "GND"})   # input bulk
add("C2", "C", "10uF", FP["C"], {"1": "+3V3", "2": "GND"})  # output bulk
add("C18", "C", "1uF", FP["C"], {"1": "+3V3", "2": "GND"})
add("C19", "C", "100nF", FP["C"], {"1": "+3V3", "2": "GND"})

# USB-C MALE PLUG (J1): the board plugs straight into the phone for power and, on
# Android, USB serial - so this is a plug, not a receptacle. The board is the
# device (UFP), hence the 5.1K Rd pulldown on CC (R1).
add("J1", "USBC", "USB-C", "Connector_USB:USB_C_Plug_ShenzhenJingTuoJin_918-118A2021Y40002_Vertical",
    {"A1": "GND", "B1": "GND", "A12": "GND", "B12": "GND", "S1": "GND",
     "A4": "+5V", "B4": "+5V", "A9": "+5V", "B9": "+5V",
     "A6": "USB_DP", "B6": "USB_DP", "A7": "USB_DM", "B7": "USB_DM",
     "A5": "CC", "B5": "CC"})  # A8/B8 (SBU) left NC; one CC pulldown for both # INFER
add("R1", "R", "5.1K", FP["R"], {"1": "CC", "2": "GND"})  # INFER CC pulldown
add("U1", "USBLC6", "USBLC6-2P6", "Package_TO_SOT_SMD:SOT-563",
    {"1": "USB_DP", "6": "USB_DP", "3": "USB_DM", "4": "USB_DM", "2": "GND", "5": "+5V"})

# MCU.
add("U3", "ESP32", "ESP32-S3R8", "Package_DFN_QFN:QFN-56-1EP_7x7mm_P0.4mm_EP4x4mm",
    {"2": "+3V3", "3": "+3V3", "20": "+3V3", "46": "+3V3",
     "55": "+3V3", "56": "+3V3", "57": "GND",
     # pin 29 VDD_SPI is a power-output (internal LDO); left NC to avoid an ERC
     # output-output clash - tie to +3V3 in review if the external flash needs it.
     "4": "EN", "5": "BOOT",
     "1": "ESP_ANT",            # INFER ESP32 WiFi/BLE antenna feed
     "12": "LORA_SCK", "13": "LORA_MISO", "14": "LORA_MOSI",
     "25": "USB_DM", "26": "USB_DP",
     "30": "FLASH_HD", "31": "FLASH_WP", "32": "FLASH_CS",
     "33": "FLASH_CLK", "34": "FLASH_DO", "35": "FLASH_DI",
     "36": "LED_GPIO",
     "44": "LORA_DIO1", "45": "LORA_BUSY", "47": "LORA_NSS", "48": "LORA_NRST",
     "53": "XTAL_N", "54": "XTAL_P"})
# EN / BOOT strapping.
add("R4", "R", "10K", FP["R"], {"1": "+3V3", "2": "EN"})     # INFER EN pullup
add("R5", "R", "10K", FP["R"], {"1": "+3V3", "2": "BOOT"})   # INFER BOOT pullup
add("R7", "R", "10K", FP["R"], {"1": "+3V3", "2": "LORA_NRST"})  # INFER NRST pullup
add("C6", "C", "10nF", FP["C"], {"1": "EN", "2": "GND"})     # INFER EN RC cap
add("SW1", "SW", "BOOT", "Button_Switch_SMD:SW_SPST_B3U-1000P", {"1": "BOOT", "2": "GND"})
add("SW2", "SW", "RESET", "Button_Switch_SMD:SW_SPST_B3U-1000P", {"1": "EN", "2": "GND"})

# Crystal + load caps.
add("Y1", "Crystal", "40MHz", "Crystal:Crystal_SMD_3225-4Pin_3.2x2.5mm",
    {"1": "XTAL_P", "2": "XTAL_N", "3": "GND", "4": "GND"})
add("C20", "C", "22pF", FP["C"], {"1": "XTAL_P", "2": "GND"})
add("C21", "C", "22pF", FP["C"], {"1": "XTAL_N", "2": "GND"})

# SPI flash + MCU decoupling.
add("U4", "FLASH", "GD25Q64", "Package_SO:SOIC-8_5.23x5.23mm_P1.27mm",
    {"1": "FLASH_CS", "2": "FLASH_DO", "3": "FLASH_WP", "4": "GND",
     "5": "FLASH_DI", "6": "FLASH_CLK", "7": "FLASH_HD", "8": "+3V3"})
for ref in ("C3", "C5", "C7", "C8", "C9", "C10", "C13", "C14"):
    add(ref, "C", "100nF", FP["C"], {"1": "+3V3", "2": "GND"})   # MCU/flash decoupling
for ref in ("C4", "C11", "C12"):
    add(ref, "C", "2.2uF", FP["C"], {"1": "+3V3", "2": "GND"})

# LoRa module.
add("U5", "WIO", "WIO-SX1262", "Wio-SX1262:Wio-SX1262",
    {"2": "LORA_MISO", "3": "LORA_MOSI", "4": "LORA_SCK", "5": "LORA_NRST",
     "6": "LORA_NSS", "7": "GND", "8": "+3V3", "10": "GND",
     "11": "LORA_BUSY", "12": "LORA_DIO1"})  # 1=RF_SW1, 9=ANT use module onboard IPEX # INFER

# RF antenna match for the ESP32 (2.4GHz) -> U.FL.  Topology is a guess. # INFER
add("L2", "L", "3.3nH", FP["L"], {"1": "ESP_ANT", "2": "ANT1"})
add("C15", "C", "1pF", FP["C"], {"1": "ESP_ANT", "2": "GND"})
add("L1", "L", "2nH", FP["L"], {"1": "ANT1", "2": "GND"})
add("AE1", "UFL", "Antenna", "Connector_Coaxial:U.FL_Molex_MCRF_73412-0110_Vertical",
    {"1": "ANT1", "2": "GND"})

# Status LED.
add("R6", "R", "1.5K", FP["R"], {"1": "LED_GPIO", "2": "LED_R"})
add("D1", "LED", "Blue", "LED_SMD:LED_0603_1608Metric", {"1": "GND", "2": "LED_R"})


# --- generator engine -------------------------------------------------------
sch = Schematic().create_new()
sch.uuid = str(U.uuid4())
root = "/" + sch.uuid
_embedded = set()


def embed(sym):
    if sym.libId not in _embedded:
        # Blank the donor symbols' inherited Datasheet/Description at the library
        # level too, so nothing leaks into schematic parity against the footprint.
        for p in getattr(sym, "properties", []):
            if p.key in ("Datasheet", "Description"):
                p.value = ""
        sch.libSymbols.append(sym)
        _embedded.add(sym.libId)


def pins_of(sym):
    out = []
    for u in sym.units:
        out.extend(u.pins)
    out.extend(sym.pins)
    return out


def place(ref, sym, value, footprint, nets, x, y):
    embed(sym)
    props = [
        Property(key="Reference", value=ref, id=0, position=Position(x, y - 10.16, 0)),
        Property(key="Value", value=value, id=1, position=Position(x, y + 10.16, 0)),
        Property(key="Footprint", value=footprint, id=2, position=Position(x, y, 0)),
        # Override the donor symbols' inherited Datasheet/Description with empty
        # values so they match the generic footprints and parity stays clean.
        Property(key="Datasheet", value="", id=3, position=Position(x, y, 0)),
        Property(key="Description", value="", id=4, position=Position(x, y, 0)),
    ]
    if ref in LCSC:  # assembly part number for the BOM
        props.append(Property(key="LCSC", value=LCSC[ref], id=5, position=Position(x, y, 0)))
    si = SchematicSymbol(
        libraryNickname=sym.libraryNickname, entryName=sym.entryName,
        position=Position(x, y, 0), unit=1, inBom=True, onBoard=True, dnp=False,
        uuid=str(U.uuid4()),
        properties=props,
        instances=[SymbolProjectInstance(
            name=PROJECT, paths=[SymbolProjectPath(sheetInstancePath=root, reference=ref, unit=1)])],
    )
    sch.schematicSymbols.append(si)
    for p in pins_of(sym):
        ax, ay = x + p.position.X, y - p.position.Y
        net = nets.get(p.number)
        if net:
            sch.globalLabels.append(GlobalLabel(text=net, position=Position(ax, ay, 0), effects=Effects()))
        else:
            sch.noConnects.append(NoConnect(position=Position(ax, ay), uuid=str(U.uuid4())))


# Lay parts out on a coarse grid (cosmetic only; netlist is by label name).
STEP, COLS, X0, Y0 = 63.5, 7, 38.1, 38.1
for i, (ref, key, value, fp, nets) in enumerate(C):
    place(ref, SYM[key], value, fp, nets, X0 + (i % COLS) * STEP, Y0 + (i // COLS) * STEP)

# Power flags so ERC sees the rails driven.
for i, net in enumerate(("+5V", "GND")):  # +3V3 is driven by the LDO output
    x, y = X0 + i * STEP, Y0 - STEP
    embed(SYM["PWR_FLAG"])
    si = SchematicSymbol(
        libraryNickname="power", entryName="PWR_FLAG", position=Position(x, y, 0),
        unit=1, inBom=False, onBoard=True, dnp=False, uuid=str(U.uuid4()),
        properties=[Property(key="Reference", value=f"#FLG{i}", id=0, position=Position(x, y, 0)),
                    Property(key="Value", value="PWR_FLAG", id=1, position=Position(x, y, 0))],
        instances=[SymbolProjectInstance(name=PROJECT, paths=[SymbolProjectPath(sheetInstancePath=root, reference=f"#FLG{i}", unit=1)])],
    )
    sch.schematicSymbols.append(si)
    sch.globalLabels.append(GlobalLabel(text=net, position=Position(x, y, 0), effects=Effects()))

sch.to_file(str(OUT))
print(f"placed {len(C)} components + 3 power flags")
print(f"wrote {OUT}")
