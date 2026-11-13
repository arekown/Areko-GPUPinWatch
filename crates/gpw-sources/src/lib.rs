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
