//! Areko GPUPinWatch core (pure, no I/O, shared by the service and the app).
//!
//! - [`model`]  : GPUs, pin sources, readings
//! - [`parse`]  : decoders for the three per-pin sources (ASUS IT8915FN, WireView Pro II, MSI Ai-TS PSU)
//! - [`policy`] : two-stage protection (throttle, then shutdown) per GPU
//! - [`config`] : user settings with safe bounds
//! - [`ipc`]    : service <-> app messages (JSON lines over a local socket / named pipe)
//!
//! Required Notice: Copyright Areko (https://github.com/arekown/Areko-GPUPinWatch)

pub mod config;
pub mod ipc;
pub mod model;
pub mod parse;
pub mod policy;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
