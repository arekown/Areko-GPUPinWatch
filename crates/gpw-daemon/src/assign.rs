//! Assigns a pin source to its GPU.
//!
//! - Source knows its GPU (`gpu_pci`, Astral chip)  -> Fixed.
//! - Exactly one GPU                                  -> Matched directly.
//! - Several GPUs: compare Σ pin current × 12 V with every GPU's NVML power over a sliding
//!   window; decide only under clear load (source > 100 W and the best GPU fits > 100 W better
//!   than the next one). Until then the source stays `Matching` (watching and warning still work).
use gpw_core::ipc::Binding;
use gpw_core::model::{norm_pci, GpuInfo, SourceInfo, PINS};
use std::collections::{HashMap, VecDeque};

/// Samples in the sliding window (5 s at 2 Hz).
pub const WINDOW: usize = 10;
/// Minimum load and minimum fit difference before a source is bound.
pub const CLEAR_W: f32 = 100.0;
const RAIL_V: f32 = 12.0;

/// One window sample: source power and the power of every GPU (same order as the GPU list).
struct Sample {
    src_w: f32,
    gpu_w: Vec<(String, f32)>,
}

#[derive(Default)]
pub struct Assigner {
    windows: HashMap<String, VecDeque<Sample>>,
    matched: HashMap<String, String>,
}

/// Result for one source; `new_match` is true on the sample that bound it.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub binding: Binding,
    pub gpu: Option<String>,
    pub new_match: bool,
}

impl Assigner {
    pub fn update(&mut self, info: &SourceInfo, amps: Option<&[f32; PINS]>, gpus: &[GpuInfo]) -> Assignment {
        if let Some(pci) = &info.gpu_pci {
            return Assignment { binding: Binding::Fixed, gpu: Some(norm_pci(pci)), new_match: false };
        }
        if let Some(pci) = self.matched.get(&info.id) {
            return Assignment { binding: Binding::Matched, gpu: Some(pci.clone()), new_match: false };
        }
        if gpus.len() == 1 {
            self.matched.insert(info.id.clone(), gpus[0].pci.clone());
            return Assignment { binding: Binding::Matched, gpu: Some(gpus[0].pci.clone()), new_match: true };
        }
        let matching = Assignment { binding: Binding::Matching, gpu: None, new_match: false };
        // A sample counts only when the source and every GPU were read.
        let (Some(amps), true) = (amps, gpus.len() > 1 && gpus.iter().all(|g| g.power_w.is_some())) else {
            return matching;
        };
        let win = self.windows.entry(info.id.clone()).or_default();
        win.push_back(Sample {
            src_w: amps.iter().sum::<f32>() * RAIL_V,
            gpu_w: gpus.iter().map(|g| (g.pci.clone(), g.power_w.unwrap_or(0.0))).collect(),
        });
        while win.len() > WINDOW {
            win.pop_front();
        }
        if win.len() < WINDOW {
            return matching;
        }
        let n = win.len() as f32;
        let load = win.iter().map(|s| s.src_w).sum::<f32>() / n;
        if load < CLEAR_W {
            return matching;
        }
        // Mean absolute difference between source power and each GPU's power.
        let mut err: Vec<(String, f32)> = gpus
            .iter()
            .map(|g| {
                let e = win
                    .iter()
                    .map(|s| s.gpu_w.iter().find(|(p, _)| *p == g.pci).map(|(_, w)| (w - s.src_w).abs()).unwrap_or(f32::MAX))
                    .sum::<f32>()
                    / n;
                (g.pci.clone(), e)
            })
            .collect();
        err.sort_by(|a, b| a.1.total_cmp(&b.1));
        if err[1].1 - err[0].1 <= CLEAR_W {
            return matching;
        }
        let pci = err[0].0.clone();
        self.windows.remove(&info.id);
        self.matched.insert(info.id.clone(), pci.clone());
        Assignment { binding: Binding::Matched, gpu: Some(pci), new_match: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpw_core::model::SourceKind;

    fn gpu(pci: &str, w: f32) -> GpuInfo {
        GpuInfo {
            pci: pci.into(),
            name: "RTX 5090".into(),
            power_w: Some(w),
            limit_w: Some(600.0),
            min_limit_w: Some(400.0),
            max_limit_w: Some(600.0),
            default_limit_w: Some(600.0),
        }
    }

    #[test]
    fn two_gpus_bind_by_correlation_only_under_clear_load() {
        let src = SourceInfo {
            id: "wireview:01".into(),
            kind: SourceKind::WireView,
            label: "WireView Pro II".into(),
            detail: String::new(),
            gpu_pci: None,
        };
        let mut a = Assigner::default();
        // Both cards idle, source idle: never decided.
        for _ in 0..2 * WINDOW {
            let r = a.update(&src, Some(&[0.4; PINS]), &[gpu("0000:02:00.0", 30.0), gpu("0000:04:00.0", 25.0)]);
            assert_eq!(r.binding, Binding::Matching);
        }
        // Card 2 under load, the source carries ~6 × 6.2 A × 12 V = 446 W.
        let mut bound = None;
        for _ in 0..WINDOW {
            let r = a.update(&src, Some(&[6.2; PINS]), &[gpu("0000:02:00.0", 32.0), gpu("0000:04:00.0", 455.0)]);
            if r.new_match {
                assert!(bound.is_none(), "bound twice");
                bound = r.gpu;
            }
        }
        assert_eq!(bound.as_deref(), Some("0000:04:00.0"));
        // Stays bound when the load moves to the other card.
        let r = a.update(&src, Some(&[0.4; PINS]), &[gpu("0000:02:00.0", 500.0), gpu("0000:04:00.0", 30.0)]);
        assert_eq!((r.binding, r.gpu.as_deref(), r.new_match), (Binding::Matched, Some("0000:04:00.0"), false));
    }
}
