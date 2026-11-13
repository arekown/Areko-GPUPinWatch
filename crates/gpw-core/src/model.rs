//! Data model shared by sources, service and app.
use serde::{Deserialize, Serialize};

/// Number of 12 V pins of a 12V-2x6 connector (the six ground pins are never measured).
pub const PINS: usize = 6;

/// One 12 V pin: current always, voltage only where the source measures it (MSI PSUs do not).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub amps: f32,
    pub volts: Option<f32>,
}

/// Where per-pin values come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// ITE IT8915FN on ASUS ROG Astral / Matrix boards (sits on the GPU, so the GPU is known).
    AstralChip,
    /// Thermal Grizzly WireView Pro II (+ Noctua Edition) between cable and card.
    WireView,
    /// MSI MPG Ai1300TS / Ai1600TS power supply (up to two 12V-2x6 connectors).
    MsiPsu,
}

/// A detected source connector. One physical device may expose several connectors (MSI PSU: 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Stable id, e.g. "astral:0000:02:00.0", "wireview:<uid hex>", "msipsu:<pid>:<connector>".
    pub id: String,
    pub kind: SourceKind,
    /// Human readable device name, e.g. "ROG Astral RTX 5090 OC", "WireView Pro II", "MPG Ai1600TS".
    pub label: String,
    /// Technical detail for the sources list, e.g. "IT8915FN · i2c-4", "ttyACM0 · FW 3", "hidraw2 · Anschluss 2".
    pub detail: String,
    /// PCI address ("0000:02:00.0") when the source itself knows its GPU (Astral). Otherwise assigned by correlation.
    pub gpu_pci: Option<String>,
}

/// An NVIDIA GPU as seen by the driver (NVML).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    /// Normalised PCI address "dddd:bb:dd.f" (lower case).
    pub pci: String,
    pub name: String,
    pub power_w: Option<f32>,
    pub limit_w: Option<f32>,
    pub min_limit_w: Option<f32>,
    pub max_limit_w: Option<f32>,
    pub default_limit_w: Option<f32>,
}

/// Normalise "00000000:02:00.0" / "0000:02:00.0" / "2:00.0" to "0000:02:00.0".
pub fn norm_pci(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    let parts: Vec<&str> = s.split(':').collect();
    let (dom, bus, rest) = match parts.as_slice() {
        [d, b, r] => (u32::from_str_radix(d, 16).unwrap_or(0), *b, *r),
        [b, r] => (0, *b, *r),
        _ => return s,
    };
    let (dev, func) = rest.split_once('.').unwrap_or((rest, "0"));
    let bus = u32::from_str_radix(bus, 16).unwrap_or(0);
    let dev = u32::from_str_radix(dev, 16).unwrap_or(0);
    let func = u32::from_str_radix(func, 16).unwrap_or(0);
    format!("{:04x}:{:02x}:{:02x}.{:x}", dom & 0xffff, bus, dev, func)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pci_forms_normalise_to_one() {
        assert_eq!(norm_pci("00000000:02:00.0"), "0000:02:00.0");
        assert_eq!(norm_pci("2:00.0"), "0000:02:00.0");
        assert_eq!(norm_pci("0000:4A:00.1"), "0000:4a:00.1");
    }
}
