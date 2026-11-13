//! Device access for the three per-pin sources (Linux + Windows). Decoding lives in gpw_core::parse.
//!
//! Safety rule for every source: only the read-only commands listed in docs/PROTOKOLLE.md are ever sent.
//!
//! Layout (one module per source, OS-specific parts in sub modules):
//!   astral.rs   + astral/linux.rs (i2c-dev ioctl)   + astral/windows.rs (nvapi64.dll I2CReadEx)
//!   wireview.rs (serial port, both OS)
//!   msipsu.rs   + msipsu/linux.rs (hidraw)          + msipsu/windows.rs (HID API + Global\MSI_PSU_Mutex)
use gpw_core::model::{GpuInfo, Pin, SourceInfo, PINS};
use thiserror::Error;

pub mod astral;
pub mod msipsu;
pub mod wireview;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("device not readable: {0}")]
    Io(String),
    #[error("bad frame: {0}")]
    Frame(#[from] gpw_core::parse::ParseError),
}

/// One reading of one connector.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub pins: [Pin; PINS],
}

/// A detected connector that can be read repeatedly. Devices with several connectors
/// (MSI Ai1600TS / Ai1300TS) appear as several `PinSource`s sharing one handle internally.
pub trait PinSource: Send {
    fn info(&self) -> &SourceInfo;
    /// Reads the six 12 V pins. Errors are expected (device unplugged, busy); the caller retries
    /// and calls [`discover`] again after repeated failures.
    fn read(&mut self) -> Result<Reading, SourceError>;
}

/// Finds every supported source on this machine. `gpus` (from NVML) lets the Astral probe
/// map I2C adapters / NVAPI handles to PCI addresses. Never fails: missing devices are just absent.
pub fn discover(gpus: &[GpuInfo]) -> Vec<Box<dyn PinSource>> {
    let mut v: Vec<Box<dyn PinSource>> = Vec::new();
    v.extend(astral::discover(gpus));
    v.extend(wireview::discover());
    v.extend(msipsu::discover());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_lists_refuse_everything_but_reads() {
        for cmd in [0x01, 0x02, 0x04] {
            assert_eq!(wireview::wv_command(cmd).unwrap(), [cmd]);
        }
        for cmd in [0x00, 0x03, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0B, 0x0C, 0x0E, 0xF0, 0xF1, 0xF2, 0xFF] {
            assert!(wireview::wv_command(cmd).is_err(), "wireview {cmd:#04x}");
        }
        assert_eq!(&msipsu::msi_command([0xFA, 0x51]).unwrap()[..3], &[0xFA, 0x51, 0]);
        for reg in gpw_core::parse::MSI_REGS_ALLOWED {
            assert_eq!(&msipsu::msi_command([0x51, reg]).unwrap()[..3], &[0x51, reg, 0]);
        }
        for cmd in [[0x50, 0xE0], [0x50, 0x10], [0x51, 0xF1], [0x51, 0xC2], [0xF1, 0x00], [0xFA, 0x50], [0x00, 0x00]] {
            assert!(msipsu::msi_command(cmd).is_err(), "msi {cmd:02x?}");
        }
    }

    /// Real hardware: run with `cargo test -p gpw-sources -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn discover_prints_sources() {
        let mut sources = discover(&[]);
        for s in sources.iter_mut() {
            println!("{} | {} | {}", s.info().id, s.info().label, s.info().detail);
            match s.read() {
                Ok(r) => {
                    let amps: Vec<String> = r.pins.iter().map(|p| format!("{:.2}", p.amps)).collect();
                    let volts: Vec<String> = r.pins.iter().map(|p| format!("{:.2}", p.volts.unwrap_or(0.0))).collect();
                    println!("  A: {}  V: {}", amps.join(" "), volts.join(" "));
                }
                Err(e) => println!("  error: {e}"),
            }
        }
        let astral = sources.iter().filter(|s| s.info().kind == gpw_core::model::SourceKind::AstralChip).count();
        println!("astral sources: {astral}");
    }
}
