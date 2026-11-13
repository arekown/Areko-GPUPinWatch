//! Service <-> app contract. Transport: one JSON object per line (UTF-8, '\n').
//! Linux: Unix socket [`SOCKET_PATH`] (mode 0666; every value is sanitised by the service).
//! Windows: named pipe [`PIPE_NAME`].
//!
//! The app sends [`Request`]s; the service answers each with one [`Reply`]. After `Subscribe`
//! the service additionally pushes a `Reply::Snapshot` after every reading until the client disconnects.
use crate::config::Config;
use crate::model::{SourceKind, PINS};
use crate::policy::Phase;
use serde::{Deserialize, Serialize};

pub const SOCKET_PATH: &str = "/run/areko-gpupinwatch/ipc.sock";
pub const PIPE_NAME: &str = r"\\.\pipe\areko-gpupinwatch";
/// Samples kept for the "highest pin" history (60 s at 2 Hz).
pub const HISTORY: usize = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    GetSnapshot,
    Subscribe,
    /// Give a throttled GPU its previous power limit back (or leave alarm-only state).
    Release { pci: String },
    SetConfig { config: Config },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Snapshot(Snapshot),
    Ok,
    Error { message: String },
}

/// How a pin source is bound to its GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    /// The source sits on the GPU (Astral chip).
    Fixed,
    /// Assigned by comparing source current with the GPUs' power draw.
    Matched,
    /// Not yet assigned (several GPUs, none under load). Watching and warning still work.
    Matching,
}

/// One card in the overview: a pin source and (when known) its GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardView {
    pub source_id: String,
    pub source_kind: SourceKind,
    pub source_label: String,
    pub binding: Binding,
    pub gpu_pci: Option<String>,
    pub gpu_name: Option<String>,
    /// 12 V pins 1..6 in A; None when the last reading failed.
    pub amps: Option<[f32; PINS]>,
    pub volts: Option<[f32; PINS]>,
    pub power_w: Option<f32>,
    pub limit_w: Option<f32>,
    /// Limit the card had before throttling (restored on release).
    pub prev_limit_w: Option<f32>,
    pub phase: Phase,
    /// 0-based pin and current that caused the last throttle/trip.
    pub trigger: Option<(usize, f32)>,
    /// Highest pin per sample, oldest first, up to [`HISTORY`] values.
    pub history_max: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceView {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    pub detail: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warn,
    Alarm,
}

/// Structured so the app can translate it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    Started,
    SourceFound { kind: SourceKind, label: String },
    SourceLost { label: String },
    Matched { label: String, gpu: String },
    Throttled { target_w: f32, pin: usize, amps: f32 },
    Released { limit_w: f32 },
    Alarm { pin: usize, amps: f32, shutdown: bool },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts_ms: u64,
    pub level: Level,
    pub gpu: Option<String>,
    pub event: EventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub poll_hz: u8,
    /// Milliseconds since the newest reading.
    pub age_ms: u64,
    pub cards: Vec<CardView>,
    pub sources: Vec<SourceView>,
    /// Newest last, at most 200.
    pub events: Vec<LogEvent>,
    pub config: Config,
}

pub fn encode<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_default();
    s.push('\n');
    s
}

pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_roundtrip_as_json_lines() {
        let r = Request::Release { pci: "0000:02:00.0".into() };
        let line = encode(&r);
        assert!(line.ends_with('\n') && line.contains("\"cmd\":\"release\""));
        assert_eq!(decode::<Request>(&line).unwrap(), r);
    }
}
