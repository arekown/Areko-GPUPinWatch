//! ASUS ROG Astral / Matrix: ITE IT8915FN on the card, read over the GPU's own I2C bus
//! (docs/PROTOKOLLE.md section 1). Detection is by the chip's answer, never by an id list.
use crate::PinSource;
use gpw_core::model::{norm_pci, GpuInfo, SourceInfo, SourceKind};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub fn discover(gpus: &[GpuInfo]) -> Vec<Box<dyn PinSource>> {
    #[cfg(target_os = "linux")]
    return linux::discover(gpus);
    #[cfg(windows)]
    return windows::discover(gpus);
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = gpus;
        Vec::new()
    }
}

/// True when the GPU at `pci` (normalised) should be probed: every GPU when NVML gave none.
fn wanted(gpus: &[GpuInfo], pci: &str) -> bool {
    gpus.is_empty() || gpus.iter().any(|g| norm_pci(&g.pci) == pci)
}

fn info(gpus: &[GpuInfo], pci: &str, detail: String) -> SourceInfo {
    let label = model_name(pci)
        .map(str::to_string)
        .or_else(|| gpus.iter().find(|g| norm_pci(&g.pci) == pci).map(|g| g.name.clone()))
        .unwrap_or_else(|| "ROG Astral".to_string());
    SourceInfo {
        id: format!("astral:{pci}"),
        kind: SourceKind::AstralChip,
        label,
        detail,
        gpu_pci: Some(pci.to_string()),
    }
}

/// Board name from the PCI subsystem id (ASUS 0x1043), for the card title. Only a label: detection
/// never depends on it. Ids as listed by LibreHardwareMonitor / PinSentinel; 89E3 verified on areko-pc-1.
fn model_name(pci: &str) -> Option<&'static str> {
    let (vendor, device) = subsystem(pci)?;
    if vendor != 0x1043 {
        return None;
    }
    Some(match device {
        0x89E3 => "ROG Astral RTX 5090 OC",
        0x89EA => "ROG Astral RTX 5090D OC",
        0x89EC => "ROG Astral LC RTX 5090",
        0x8A2E => "ROG Astral RTX 5090 OC White",
        0x8A5A | 0x8A3C => "ROG Astral RTX 5090 BTF OC",
        0x8A61 => "ROG Matrix RTX 5090",
        0x89DE => "ROG Astral RTX 5080 OC",
        0x89DF => "ROG Astral RTX 5080",
        0x8A2B => "ROG Astral RTX 5080 OC White",
        0x8A45 => "ROG Astral RTX 5080 OC Hatsune Miku",
        _ => return None,
    })
}

#[cfg(target_os = "linux")]
fn subsystem(pci: &str) -> Option<(u16, u16)> {
    let read = |f: &str| {
        let s = std::fs::read_to_string(format!("/sys/bus/pci/devices/{pci}/{f}")).ok()?;
        u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
    };
    Some((read("subsystem_vendor")?, read("subsystem_device")?))
}

#[cfg(not(target_os = "linux"))]
fn subsystem(_pci: &str) -> Option<(u16, u16)> {
    None // Windows: falls back to the NVML name
}
