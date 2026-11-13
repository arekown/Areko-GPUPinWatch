# Protokolle der Pin-Quellen

Nur Fakten, kein übernommener Code. Decoder: `crates/gpw-core/src/parse.rs`.
Quellen je Abschnitt; Rechercheprotokoll in Plane PINW-2.

## 1. ASUS ROG Astral / Matrix – ITE IT8915FN auf der Karte

- 7-Bit-I2C-Adresse **0x2B**, Register **0x80**, **24 Byte**: 6 × (u16 BE mV, u16 BE mA), Reihenfolge umgekehrt (Bytes 0–3 = Pin 6).
- Plausibel: 6–16 V, ≤ 30 A je Pin; Auflösung 8 mV / 20 mA. Lesen stört GPU Tweak III nicht.
- Erkennung **über die Chip-Antwort**, nicht über eine ID-Liste. Bekannte Subsystem-IDs 1043:… 89E3 (5090 OC, an areko-pc-1 nachgewiesen), 89EA, 89EC, 8A2E, 8A61, 8A5A, 8A3C, 89DE, 89DF, 8A2B, 8A45.

**Linux:** Bus = Adapter mit Namen `NVIDIA i2c adapter 1 at <bus>:<dev>.<fn>` (`/sys/bus/i2c/devices/i2c-*/name`, Nummern ändern sich je Boot); `/dev/i2c-N` öffnen, `I2C_SLAVE` 0x2B, ein SMBus-I2C-Blockread (`I2C_SMBUS`, Größe `I2C_SMBUS_I2C_BLOCK_DATA` = 8, `block[0]` = 24). Braucht `i2c-dev`. Jeder Lesevorgang wartet im NVIDIA-Treiber ≈ 7 ms aktiv → höchstens 2–4 Hz. Quellen: LACT Issue #906, vhpwr-guard, eigene Messung areko-pc-1.

**Windows (NVAPI, `nvapi64.dll`):** Export `nvapi_QueryInterface(u32)`. IDs: Initialize `0x0150E828`, EnumPhysicalGPUs `0xE5AC921F`, GPU_GetPCIIdentifiers `0x2DDFB66E`, GPU_GetBusId `0x1BE0B8E5`, GPU_GetBusSlotId `0x2A0A350F`, **I2CReadEx `0x4D7B0709`** (undokumentiert; Signatur `(handle, NV_I2C_INFO*, u32* = 0)`). Nie I2CWriteEx.
`NV_I2C_INFO_V3` (x64, 64 Byte, version `0x00030040`): 0 version u32 · 4 displayMask u32 = 0 · 8 bIsDDCPort u8 = 0 · 9 i2cDevAddress u8 = **0x56** · 16 pbI2cRegAddress ptr → [0x80] · 24 regAddrSize u32 = 1 · 32 pbData ptr · 40 cbSize u32 = 24 · 44 i2cSpeed u32 = 0xFFFF · 48 i2cSpeedKhz u32 = 4 (100 kHz) · 52 portId u8 = 1 · 56 bIsPortIdSet u32 = 1.
GPU ↔ PCI: GetBusId = Bus, GetBusSlotId = Device. Kein Admin nötig. Quellen: NVIDIA/nvapi (MIT), LibreHardwareMonitor PR #2168 (MPL-2.0), PinSentinel (MIT), Timic3/astral-power-monitoring (Unlicense).

## 2. Thermal Grizzly WireView Pro II (+ Noctua Edition)

- USB CDC-ACM **0483:5740**, 115200 8N1, keine Flusssteuerung, Timeout 1 s. Windows: normaler COM-Port. Port **exklusiv** öffnen (Linux `TIOCEXCL`); die TG-App darf nicht gleichzeitig laufen.
- Begrüßung: öffnen, Eingang leeren, RTS setzen → 32 B `Thermal Grizzly WireView Pro II\0` (kann beim Wiederöffnen ausbleiben → trotzdem weiter).
- Kein Rahmen, keine Prüfsumme: Befehlsbyte senden, feste Länge lesen; **vor jedem Befehl Eingang leeren**, jeden Rahmen plausibilisieren.
- **Erlaubt (nur lesen):** `0x01` → 3 B [0xEF, 0x05 Pro II | 0x06 Noctua, FW]; `0x02` → 12 B UID; `0x04` → 100 B Live. 0x07/0x08 (WireView II) = andere Familie, nicht unterstützt.
- **Nie senden:** 0x06, 0x08, 0x09, 0x0B, 0x0C, 0x0E, 0xF0, **0xF1 (Bootloader)**, 0xF2, sowie alles Unbekannte.
- Live-Rahmen (LE): 0/2/4/6 i16 Temp ÷10 °C (−1000 = fehlt) · 8 u16 VDD mV · 10 u8 Lüfter % · 11 Füllbyte 0 · Pin n: 12+12n i16 mV, 16+12n u32 mA, 20+12n u32 mW · 84 u32 Σ mW · 88 u32 Σ mA · 92 u16 Ø mV · 94 u8 Kabel (0=600 W…3=150 W) · 95 Füllbyte 0 · 96 u16 aktive Fehler · 98 u16 protokollierte Fehler.
- Takt 500–1000 ms (≥ 100 ms). Quellen: Gustav0ar/wireview-pro-ii `docs/protocol.md` (MIT), twolven/wireview-monitor (MIT).

## 3. MSI MPG Ai1300TS / Ai1600TS (GPU Safeguard+)

- USB-HID **VID 0x0DB0**, PID **0xAA6F** (Ai1300TS), **0x808C** (Ai1600TS); Usage Page 0xFF00, keine Report-IDs, 64-Byte-Reports.
- Linux hidraw: Gerät über `HID_ID=0003:00000DB0:0000XXXX` in `/sys/class/hidraw/*/device/uevent` finden, lesend+schreibend öffnen, **65 B schreiben (0x00 + 64)**, 64 B lesen.
- Windows: HID öffnen (kein Admin); MSI Center/Afterburner/HWiNFO teilen den Mutex `Global\MSI_PSU_Mutex` – vor jeder Anfrage nehmen (`OpenMutex(SYNCHRONIZE)`, fehlt er: anlegen), danach freigeben.
- Einmal je Verbindung **`FA 51`** (Antwort FA 51, Byte 3 ≠ FE). Lesen: **`51 <reg>`**, Antwort-Echo `51 <reg>`, Daten ab Byte 2. Vor jeder Anfrage Eingang leeren, nur Antwort mit passendem Echo (Frist 500 ms), `FE` + Nullen = belegt.
- **Erlaubt:** FA 51 und 51 mit 10, 11, 12, 13, C0, C1, E0, E1. **Nie:** Opcode 0x50 (Schreiben), Register F1 (speichern), C2 (Summer).
- Reg **E0** (u16 LE, LINEAR11: Exponent 5 Bit mit Vorzeichen, Mantisse 11 Bit ohne, A): 0/2/4 Schienen 12/5/3,3 V · **6–17 Anschluss 1 Pin 1–6 · 18–29 Anschluss 2 Pin 1–6** · 30 Leistung W. Keine Pin-Spannungen. Unbenutzter Anschluss = 0 A. Auflösung 0,0625 A.
- Reg C1: Status je Anschluss (0 ok, 1 Überstrom, 2 Ungleichgewicht, 3 > 18 A). Reg 11: Modellname.
- Offen: welche Buchse „Anschluss 1“ ist → Zuordnung zur GPU über Lastvergleich. MAG A1000PLS/A1200PLS liefern keine USB-Daten.
- Quellen: DeniDoman/meltalarm, ImtyOSS/psutop, HesiPullupTimbo/msi-psu-monitor (alle MIT).
