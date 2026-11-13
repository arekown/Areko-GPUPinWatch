# Architektur

```
crates/gpw-core     rein, ohne I/O: Datenmodell, Decoder, Schutzlogik (PinGuard), Einstellungen, IPC-Vertrag
crates/gpw-sources  Gerätezugriff Linux + Windows, je Quelle ein Modul (Trait PinSource, discover())
crates/gpw-daemon   Dienst „areko-gpupinwatchd“ (root / Windows-Dienst): Messschleife, NVML, Drosseln,
                    Herunterfahren, Zuordnung Quelle↔GPU, Zustand, IPC-Server, systemd-Unit/Installer
app/                Fenster „Areko GPUPinWatch“ (Tauri 2, HTML/CSS/JS aus dem abgenommenen Entwurf):
                    Übersicht, Einstellungen, Ereignisse, Info; Taskleisten-Symbol; Desktop-Alarm; IPC-Client
```

## Regeln für jeden Code

- Pro Quelle/Aufgabe ein Modul; OS-Teile in `linux.rs`/`windows.rs` mit `#[cfg(target_os = …)]`.
- Geräten wird **nur** gesendet, was `docs/PROTOKOLLE.md` erlaubt (Erlaubtliste im Code, alles andere abweisen).
- Kein fremder Code, nur Protokollfakten. Abhängigkeiten nur mit MIT/Apache/BSD/Zlib-artiger Lizenz.
- Kommentare englisch, Oberflächentexte über die Übersetzungstabelle (7 Sprachen).
- Tests nur für Logik, die still kaputtgehen kann (Decoder, Schutzlogik, Zuordnung, IPC).

## Ablauf im Dienst (je Takt, Standard 2 Hz)

1. NVML: GPUs (PCI, Name, Leistung, Grenze, min/max/Standard-Grenze).
2. Jede Quelle lesen → `PinGuard::step` je Karte.
3. `Throttle` → Grenze merken, `nvmlDeviceSetPowerManagementLimit` auf `Config::throttle_watts`.
   `Trip` → Ereignis + Protokolldatei, dann je Einstellung herunterfahren oder nur Alarm.
4. Zuordnung WireView/MSI-Anschluss → GPU: bei einer GPU direkt; sonst Korrelation Σ Pin-Strom × 12 V mit
   NVML-Leistung über ein gleitendes Fenster, erst bei klarer Last (> 100 W Unterschied) festlegen.
5. Snapshot an abonnierte Clients; Zustand nach `/run/areko-gpupinwatch/state.json` (Linux) bzw.
   `%ProgramData%\Areko GPUPinWatch\state.json`, damit ein Neustart des Dienstes die alte Grenze zurückgeben kann.
