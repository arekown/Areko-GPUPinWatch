//! Thermal Grizzly WireView Pro II (+ Noctua Edition) on its USB serial port, both OS
//! (docs/PROTOKOLLE.md section 2). Every byte sent goes through [`wv_command`].
use crate::{PinSource, Reading, SourceError};
use gpw_core::model::{SourceInfo, SourceKind};
use gpw_core::parse::{
    wireview_live, wireview_supported, WIREVIEW_PID, WIREVIEW_VID, WV_CMD_IDENTITY, WV_CMD_LIVE, WV_CMD_UID,
    WV_IDENTITY_LEN, WV_LIVE_LEN, WV_UID_LEN,
};
use serialport::{ClearBuffer, DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::io::Read;
use std::time::Duration;

const BAUD: u32 = 115_200;
const TIMEOUT: Duration = Duration::from_secs(1);
const GREETING_LEN: usize = 32;

/// The allow-list: the only commands ever sent to a WireView (all read-only).
/// Returns the bytes to send, or refuses anything else (0xF1 = bootloader, 0x06 = write, ...).
pub(crate) fn wv_command(cmd: u8) -> Result<[u8; 1], SourceError> {
    match cmd {
        WV_CMD_IDENTITY | WV_CMD_UID | WV_CMD_LIVE => Ok([cmd]),
        _ => Err(SourceError::Io(format!("wireview command {cmd:#04x} refused"))),
    }
}

fn io_err(e: impl std::fmt::Display) -> SourceError {
    SourceError::Io(e.to_string())
}

/// Clears the input, sends one allowed command and reads the fixed-length answer.
fn transact(port: &mut dyn SerialPort, cmd: u8, len: usize) -> Result<Vec<u8>, SourceError> {
    let bytes = wv_command(cmd)?;
    port.clear(ClearBuffer::Input).map_err(io_err)?;
    port.write_all(&bytes).map_err(io_err)?;
    port.flush().map_err(io_err)?;
    let mut buf = vec![0u8; len];
    port.read_exact(&mut buf).map_err(io_err)?;
    Ok(buf)
}

/// WireView ports as (path to open, short name for the detail line).
#[cfg(target_os = "linux")]
fn find_ports() -> Vec<(String, String)> {
    use std::fs;
    let hex = |p: &std::path::Path, f: &str| {
        fs::read_to_string(p.join(f)).ok().and_then(|s| u16::from_str_radix(s.trim(), 16).ok())
    };
    let mut v = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/tty") else { return v };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("ttyACM") {
            continue;
        }
        // tty -> device (USB interface) -> parents up to the USB device with idVendor/idProduct
        let Ok(dev) = fs::canonicalize(e.path().join("device")) else { continue };
        let usb = dev.ancestors().take(3).find(|p| p.join("idVendor").exists());
        if let Some(usb) = usb {
            if hex(usb, "idVendor") == Some(WIREVIEW_VID) && hex(usb, "idProduct") == Some(WIREVIEW_PID) {
                v.push((format!("/dev/{name}"), name));
            }
        }
    }
    v.sort();
    v
}

#[cfg(not(target_os = "linux"))]
fn find_ports() -> Vec<(String, String)> {
    let Ok(ports) = serialport::available_ports() else { return Vec::new() };
    ports
        .into_iter()
        .filter(|p| {
            matches!(&p.port_type, serialport::SerialPortType::UsbPort(u) if u.vid == WIREVIEW_VID && u.pid == WIREVIEW_PID)
        })
        .map(|p| (p.port_name.clone(), p.port_name))
        .collect()
}

struct Opened {
    port: Box<dyn SerialPort>,
    identity: Vec<u8>,
    uid: Vec<u8>,
}

/// Opens the port exclusively, takes the greeting and checks identity (0x01) and UID (0x02).
fn open(path: &str) -> Result<Opened, SourceError> {
    let builder = serialport::new(path, BAUD)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(TIMEOUT);
    #[cfg(unix)]
    let builder = builder.exclusive(true); // TIOCEXCL; COM ports are exclusive on Windows anyway
    let mut port = builder.open().map_err(io_err)?;
    port.clear(ClearBuffer::Input).map_err(io_err)?;
    port.write_request_to_send(true).map_err(io_err)?;
    // greeting "Thermal Grizzly WireView Pro II\0"; may be missing when the port is reopened
    let mut greeting = [0u8; GREETING_LEN];
    let _ = port.read_exact(&mut greeting);
    let identity = transact(port.as_mut(), WV_CMD_IDENTITY, WV_IDENTITY_LEN)?;
    if !wireview_supported(&identity) {
        return Err(SourceError::Io(format!("{path}: unsupported identity {identity:02x?}")));
    }
    let uid = transact(port.as_mut(), WV_CMD_UID, WV_UID_LEN)?;
    Ok(Opened { port, identity, uid })
}

fn source_info(o: &Opened, name: &str) -> SourceInfo {
    let label = if o.identity[1] == 0x06 { "WireView Pro II Noctua Edition" } else { "WireView Pro II" };
    let uid: String = o.uid.iter().map(|b| format!("{b:02x}")).collect();
    SourceInfo {
        id: format!("wireview:{uid}"),
        kind: SourceKind::WireView,
        label: label.to_string(),
        detail: format!("{name} · FW {}", o.identity[2]),
        gpu_pci: None,
    }
}

struct WireView {
    info: SourceInfo,
    uid: Vec<u8>,
    port: Option<Box<dyn SerialPort>>,
}

impl WireView {
    /// After an error: find the same device again (by UID), the port name may have changed.
    fn reconnect(&mut self) -> Result<(), SourceError> {
        for (path, name) in find_ports() {
            if let Ok(o) = open(&path) {
                if o.uid == self.uid {
                    self.info = source_info(&o, &name);
                    self.port = Some(o.port);
                    return Ok(());
                }
            }
        }
        Err(SourceError::Io(format!("{} not found", self.info.id)))
    }
}

impl PinSource for WireView {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self) -> Result<Reading, SourceError> {
        if self.port.is_none() {
            self.reconnect()?;
        }
        let port = self.port.as_mut().expect("connected above");
        match transact(port.as_mut(), WV_CMD_LIVE, WV_LIVE_LEN) {
            Ok(b) => Ok(Reading { pins: wireview_live(&b)?.pins }),
            Err(e) => {
                self.port = None;
                Err(e)
            }
        }
    }
}

pub fn discover() -> Vec<Box<dyn PinSource>> {
    let mut v: Vec<Box<dyn PinSource>> = Vec::new();
    for (path, name) in find_ports() {
        if let Ok(o) = open(&path) {
            let info = source_info(&o, &name);
            v.push(Box::new(WireView { info, uid: o.uid, port: Some(o.port) }));
        }
    }
    v
}
