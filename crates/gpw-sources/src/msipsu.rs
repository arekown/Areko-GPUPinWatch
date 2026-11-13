//! MSI MPG Ai1300TS / Ai1600TS (GPU Safeguard+) over USB HID (docs/PROTOKOLLE.md section 3).
//! One PSU = two `PinSource`s (connector 1 and 2) sharing one handle. Every report sent goes
//! through [`msi_command`].
use crate::{PinSource, Reading, SourceError};
use gpw_core::model::{Pin, SourceInfo, SourceKind, PINS};
use gpw_core::parse::{
    msi_data, msi_pins, msi_request, MsiPins, MSI_HANDSHAKE, MSI_PID_AI1300TS, MSI_PID_AI1600TS, MSI_READ, MSI_VID,
    MSI_REGS_ALLOWED, MSI_REG_PINS, MSI_REPORT,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as os;

/// Deadline for the reply with the matching echo.
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
/// A register E0 reading this young is reused for the other connector.
const REUSE: Duration = Duration::from_millis(100);

/// The allow-list: the only reports ever sent to the PSU. `FA 51` (handshake) and `51 <reg>`
/// for the registers in `MSI_REGS_ALLOWED`; refuses opcode 0x50 (write), F1 (store), C2 (buzzer), ...
pub(crate) fn msi_command(cmd: [u8; 2]) -> Result<[u8; MSI_REPORT], SourceError> {
    if cmd == MSI_HANDSHAKE {
        let mut r = [0u8; MSI_REPORT];
        r[..2].copy_from_slice(&cmd);
        return Ok(r);
    }
    if cmd[0] == MSI_READ && MSI_REGS_ALLOWED.contains(&cmd[1]) {
        return Ok(msi_request(cmd[1]));
    }
    Err(SourceError::Io(format!("msi command {:02x} {:02x} refused", cmd[0], cmd[1])))
}

fn io_err(e: impl std::fmt::Display) -> SourceError {
    SourceError::Io(e.to_string())
}

fn model(pid: u16) -> &'static str {
    match pid {
        MSI_PID_AI1300TS => "MPG Ai1300TS",
        _ => "MPG Ai1600TS",
    }
}

fn supported(vid: u16, pid: u16) -> bool {
    vid == MSI_VID && (pid == MSI_PID_AI1300TS || pid == MSI_PID_AI1600TS)
}

/// "FE" followed by zeros: the PSU is busy.
fn is_busy(r: &[u8]) -> bool {
    r[0] == 0xFE && r[1..].iter().all(|&x| x == 0)
}

/// One PSU, shared by its two connector sources.
struct Psu {
    pid: u16,
    hid: Option<os::Hid>,
    last: Option<(Instant, MsiPins)>,
}

impl Psu {
    /// Sends one allowed command (input cleared first, under the PSU lock) and waits for the
    /// reply with the matching echo, or a busy reply.
    fn query(&mut self, cmd: [u8; 2]) -> Result<[u8; MSI_REPORT], SourceError> {
        let report = msi_command(cmd)?;
        let hid = self.hid.as_mut().ok_or_else(|| SourceError::Io("not connected".into()))?;
        let _lock = hid.lock().map_err(io_err)?;
        hid.drain();
        hid.write(&report).map_err(io_err)?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(SourceError::Io(format!("no reply to {:02x} {:02x}", cmd[0], cmd[1])));
            }
            if let Some(r) = hid.read(left).map_err(io_err)? {
                if r[..2] == cmd || is_busy(&r) {
                    return Ok(r);
                }
            }
        }
    }

    /// Reopens the PSU after an error (searched again by PID, the device node may change).
    fn connect(&mut self) -> Result<(), SourceError> {
        if self.hid.is_some() {
            return Ok(());
        }
        let found = os::find()
            .into_iter()
            .find(|f| f.pid == self.pid)
            .ok_or_else(|| SourceError::Io(format!("msi psu {:04x} not found", self.pid)))?;
        self.open(&found.path)
    }

    /// Opens the device and sends `FA 51` once per connection.
    fn open(&mut self, path: &str) -> Result<(), SourceError> {
        self.hid = Some(os::Hid::open(path).map_err(io_err)?);
        let ok = match self.query(MSI_HANDSHAKE) {
            Ok(r) => r[..2] == MSI_HANDSHAKE && r[3] != 0xFE,
            Err(_) => false,
        };
        if !ok {
            self.hid = None;
            return Err(SourceError::Io("msi handshake failed".into()));
        }
        Ok(())
    }

    fn pins(&mut self) -> Result<MsiPins, SourceError> {
        if let Some((t, p)) = &self.last {
            if t.elapsed() < REUSE {
                return Ok(p.clone());
            }
        }
        self.connect()?;
        let reply = match self.query([MSI_READ, MSI_REG_PINS]) {
            Ok(r) => r,
            Err(e) => {
                self.hid = None; // reopen and handshake again on the next read
                return Err(e);
            }
        };
        let p = msi_pins(msi_data(&reply, MSI_REG_PINS)?)?;
        self.last = Some((Instant::now(), p.clone()));
        Ok(p)
    }
}

struct Connector {
    info: SourceInfo,
    index: usize,
    psu: Arc<Mutex<Psu>>,
}

impl PinSource for Connector {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self) -> Result<Reading, SourceError> {
        let p = self.psu.lock().map_err(|_| SourceError::Io("msi psu lock poisoned".into()))?.pins()?;
        let mut pins = [Pin { amps: 0.0, volts: None }; PINS];
        for (pin, &a) in pins.iter_mut().zip(p.connectors[self.index].iter()) {
            pin.amps = a; // the PSU measures no pin voltages
        }
        Ok(Reading { pins })
    }
}

pub fn discover() -> Vec<Box<dyn PinSource>> {
    let mut v: Vec<Box<dyn PinSource>> = Vec::new();
    for f in os::find() {
        let mut psu = Psu { pid: f.pid, hid: None, last: None };
        if psu.open(&f.path).is_err() {
            continue;
        }
        let psu = Arc::new(Mutex::new(psu));
        for index in 0..2 {
            let n = index + 1;
            v.push(Box::new(Connector {
                info: SourceInfo {
                    id: format!("msipsu:{:04x}:{n}", f.pid),
                    kind: SourceKind::MsiPsu,
                    label: model(f.pid).to_string(),
                    detail: format!("{} · Anschluss {n}", f.name),
                    gpu_pci: None,
                },
                index,
                psu: psu.clone(),
            }));
        }
    }
    v
}
