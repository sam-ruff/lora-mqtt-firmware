# Component datasheets - key specs

Datasheets for the main active parts, the LoRa module and the user-input switches
on the MESHTASTIC NODE board, with the points that matter for firmware bring-up
and board iteration. The PDFs in this folder are authoritative; this is a digest.

Generic 0603 passives and the mechanical connectors (U.FL AE1, GT-USB-8016B
USB-C) are not stored here - they are jellybean/obscure parts without useful
datasheets. See `../Production Files/BOM/` for their LCSC part numbers.

| Ref | Part | Role | Package | Datasheet |
| --- | --- | --- | --- | --- |
| U3 | ESP32-S3R8 | MCU | QFN-56 7x7 | `ESP32-S3_datasheet.pdf` |
| U5 | Wio-SX1262 | LoRa module | 12-pin SMT module | `Wio-SX1262_module_datasheet.pdf` |
| (U5 core) | Semtech SX1262 | LoRa radio IC | (inside module) | `SX1262_datasheet.pdf` |
| U4 | GD25Q64E | SPI NOR flash | SOIC-8 | `GD25Q64E_datasheet.pdf` |
| U2 | TLV75901 (TLV759P) | 3V3 LDO | WSON-6 2x2 | `TLV75901_TLV759P_datasheet.pdf` |
| U1 | USBLC6-2P6 | USB ESD protection | SOT-563 | `USBLC6-2P6_datasheet.pdf` |
| SW1/SW2 | Omron B3U-1000P | Tactile switches | SMD | `B3U-1000P_tactile_switch_datasheet.pdf` |

---

## U3 - Espressif ESP32-S3R8 (MCU)

- Dual-core Xtensa LX7, up to 240 MHz; 512 KB SRAM on-chip.
- `R8` suffix: 8 MB octal SPI PSRAM in-package. No internal flash, so flash is
  external (see U4).
- 2.4 GHz Wi-Fi (802.11 b/g/n) and Bluetooth 5 LE.
- Native USB OTG (full-speed) - this is what J1/U1 connect to; no USB-UART bridge
  chip is needed for flashing/serial.
- ~45 programmable GPIO, multiple SPI/I2C/UART, ADC.
- Needs an external 40 MHz crystal (Y1 here) and external SPI flash.
- Package QFN-56, 7x7 mm.

## U5 - Seeed Wio-SX1262 (LoRa module)  [verified from datasheet]

- Pure-RF module built around the Semtech SX1262 (see below for the silicon).
- Module size 11.6 x 11 x 2.95 mm, 12-pin SMT.
- TX power up to +22 dBm, HF band 862-930 MHz (this is the EU868/US915-class
  variant).
- RX sensitivity -136.73 dBm at SF12 / 125 kHz BW (incl. line loss).
- Onboard RF: default IPEX (U.FL) port and integrated matching/filter + RF switch.
- Integrated 32 MHz active TCXO as the RF reference; DIO3 supplies the TCXO -
  firmware must configure DIO3 as the TCXO supply (not XTAL mode).
- Uses an internal DC-DC supply scheme.
- Host interface is SPI plus control lines: NSS, SCK, MOSI, MISO, BUSY, DIO1
  (IRQ), RESET (and DIO2 used internally for the RF switch).
- Sleep current as low as 1.62 uA.
- Note: the module already integrates matching and its own U.FL. The board also
  carries an AE1 U.FL plus L1/L2/C15 - the exact RF routing/antenna selection is
  not confirmed without the schematic.

## SX1262 (radio IC inside the module)

- Semtech sub-GHz LoRa/(G)FSK transceiver, 150 MHz-960 MHz.
- LoRa and Long Range FHSS plus (G)FSK; LoRa BW 7.8-500 kHz.
- TX up to +22 dBm (high-power PA); RX current ~4.6 mA.
- SPI control; the host talks to it via the module pins above.
- Stored mainly as reference for register-level/driver work; for this board the
  module datasheet governs the electricals.

## U4 - GigaDevice GD25Q64E (SPI NOR flash)

- 64 Mbit (8 MByte) serial NOR flash - this holds the ESP32-S3 firmware image.
- Standard SPI plus Dual and Quad I/O; up to 133 MHz; SPI modes 0 and 3.
- Supply 2.7-3.6 V (runs from the 3V3 rail).
- 4 KB sectors, 32/64 KB blocks; software reset, security/OTP registers, unique ID.
- Package SOIC-8 (5.3 x 5.3 mm).

## U2 - TI TLV75901 / TLV759P (3V3 LDO)  [verified from datasheet]

- Adjustable 1 A LDO, WSON-6 (DRV) 2x2 mm.
- Input range 1.5-6.0 V (here fed from USB 5 V); adjustable output 0.55-5.5 V.
- Feedback reference 0.55 V. The board's divider R2 = 100 K (top) / R3 = 20 K
  (bottom) sets Vout = 0.55 x (1 + 100/20) = 3.3 V.
- Very low dropout: 225 mV max at 1 A (3.3 V out); accuracy 0.7% typ.
- Iq ~25 uA; built-in soft-start, active output discharge, thermal shutdown,
  current limit and UVLO.
- Has an enable pin - check whether it is tied to VIN (always-on) or to a GPIO.

## U1 - STMicroelectronics USBLC6-2P6 (USB ESD protection)

- Very-low-capacitance ESD/TVS array for protecting two data lines (USB 2.0 D+/D-).
- Sits between the USB-C connector (J1) and the ESP32-S3 native USB pins.
- Low line capacitance (~3.5 pF typ) so it does not degrade USB 2.0 signal
  integrity; IEC 61000-4-2 air/contact ESD rated.
- Package SOT-563 on this board (ST also lists SOT-666 for the P6).

## SW1 / SW2 - Omron B3U-1000P (tactile switches)

- Ultra-small SMD tactile switches; on an ESP32-S3 board these are almost
  certainly BOOT (IO0) and RESET/EN.
- Operating force ~1.5 N (about 150 gf); travel ~0.15 mm.
- Rated 50 mA at 12 VDC (signal-level use here).

---

## Sources

- ESP32-S3: https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf
- Wio-SX1262 module: https://files.seeedstudio.com/products/SenseCAP/Wio_SX1262/Wio-SX1262_Module_Datasheet.pdf
- SX1262: https://cdn.sparkfun.com/assets/6/b/5/1/4/SX1262_datasheet.pdf
- GD25Q64E: https://uploadcdn.oneyac.com/attachments/files/brand_pdf/gigadevice/01/5B/GD25Q64ESIGR.pdf
- TLV759P: https://www.ti.com/lit/ds/symlink/tlv759p.pdf
- USBLC6-2P6: https://datasheet.octopart.com/USBLC6-2P6-STMicroelectronics-datasheet-7828165.pdf (official: https://www.st.com/resource/en/datasheet/usblc6-2.pdf)
- B3U-1000P: https://omronfs.omron.com/en_US/ecb/products/pdf/en-b3u.pdf
