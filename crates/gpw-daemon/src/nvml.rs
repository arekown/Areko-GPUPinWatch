//! NVIDIA GPUs via NVML: list, power draw, power limit (current/min/max/default), set the limit.
use crate::guard::GpuControl;
use gpw_core::model::{norm_pci, GpuInfo};
use nvml_wrapper::Nvml;

pub struct NvmlControl {
    nvml: Nvml,
}

impl NvmlControl {
    pub fn new() -> Result<Self, String> {
        Nvml::init().map(|nvml| NvmlControl { nvml }).map_err(|e| format!("NVML: {e}"))
    }
}

fn watts(mw: Result<u32, nvml_wrapper::error::NvmlError>) -> Option<f32> {
    mw.ok().map(|v| v as f32 / 1000.0)
}

impl GpuControl for NvmlControl {
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, String> {
        let n = self.nvml.device_count().map_err(|e| format!("NVML: {e}"))?;
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n {
            let d = self.nvml.device_by_index(i).map_err(|e| format!("NVML GPU {i}: {e}"))?;
            let pci = d.pci_info().map_err(|e| format!("NVML GPU {i}: {e}"))?.bus_id;
            let limits = d.power_management_limit_constraints().ok();
            v.push(GpuInfo {
                pci: norm_pci(&pci),
                name: d.name().unwrap_or_else(|_| format!("GPU {i}")),
                power_w: watts(d.power_usage()),
                limit_w: watts(d.power_management_limit()),
                min_limit_w: limits.as_ref().map(|l| l.min_limit as f32 / 1000.0),
                max_limit_w: limits.as_ref().map(|l| l.max_limit as f32 / 1000.0),
                default_limit_w: watts(d.power_management_limit_default()),
            });
        }
        Ok(v)
    }

    fn set_limit(&mut self, pci: &str, watts: f32) -> Result<(), String> {
        let mut d = self.nvml.device_by_pci_bus_id(pci).map_err(|e| format!("NVML {pci}: {e}"))?;
        d.set_power_management_limit((watts * 1000.0).round() as u32).map_err(|e| format!("NVML {pci}: {e}"))
    }
}
