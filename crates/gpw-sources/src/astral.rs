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
    let label = gpus
        .iter()
        .find(|g| norm_pci(&g.pci) == pci)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "ROG Astral".to_string());
    SourceInfo {
        id: format!("astral:{pci}"),
        kind: SourceKind::AstralChip,
        label,
        detail,
        gpu_pci: Some(pci.to_string()),
    }
}
