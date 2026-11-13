//! Linux: i2c-dev on the NVIDIA adapter "NVIDIA i2c adapter 1 at <pci>", one SMBus I2C block read.
use super::{info, wanted};
use crate::{PinSource, Reading, SourceError};
use gpw_core::model::{norm_pci, GpuInfo, SourceInfo};
use gpw_core::parse::{astral_block, ASTRAL_ADDR, ASTRAL_LEN, ASTRAL_REG};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;

// linux/i2c-dev.h, linux/i2c.h
const I2C_SLAVE: u32 = 0x0703;
const I2C_SMBUS: u32 = 0x0720;
const I2C_SMBUS_READ: u8 = 1;
const I2C_SMBUS_I2C_BLOCK_DATA: u32 = 8;
const I2C_SMBUS_BLOCK_MAX: usize = 32;
const ADAPTER_PREFIX: &str = "NVIDIA i2c adapter 1 at ";

/// struct i2c_smbus_ioctl_data
#[repr(C)]
struct SmbusIoctlData {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut [u8; I2C_SMBUS_BLOCK_MAX + 2],
}

/// NVIDIA adapters 1 as (bus number, normalised PCI address), sorted by PCI address.
/// Bus numbers change between boots, so this is searched again after every read error.
fn adapters() -> Vec<(u32, String)> {
    let mut v = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/bus/i2c/devices") else { return v };
    for e in dir.flatten() {
        let file_name = e.file_name();
        let Some(bus) = file_name.to_str().and_then(|n| n.strip_prefix("i2c-")).and_then(|n| n.parse().ok()) else {
            continue;
        };
        let Ok(name) = fs::read_to_string(e.path().join("name")) else { continue };
        if let Some(addr) = name.trim().strip_prefix(ADAPTER_PREFIX) {
            v.push((bus, norm_pci(addr)));
        }
    }
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

fn open_bus(bus: u32) -> io::Result<File> {
    let f = OpenOptions::new().read(true).write(true).open(format!("/dev/i2c-{bus}"))?;
    // SAFETY: plain ioctl on an fd we own; the argument is the 7-bit address by value.
    if unsafe { libc::ioctl(f.as_raw_fd(), I2C_SLAVE as _, ASTRAL_ADDR as libc::c_ulong) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(f)
}

/// One SMBus "I2C block read" of the 24-byte pin block (the only transfer made on this bus).
fn read_block(f: &File) -> io::Result<[u8; ASTRAL_LEN]> {
    let mut block = [0u8; I2C_SMBUS_BLOCK_MAX + 2];
    block[0] = ASTRAL_LEN as u8;
    let mut args =
        SmbusIoctlData { read_write: I2C_SMBUS_READ, command: ASTRAL_REG, size: I2C_SMBUS_I2C_BLOCK_DATA, data: &mut block };
    // SAFETY: `args` and `block` outlive the call; the kernel writes at most block[0..=32].
    if unsafe { libc::ioctl(f.as_raw_fd(), I2C_SMBUS as _, &mut args as *mut SmbusIoctlData) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = [0u8; ASTRAL_LEN];
    out.copy_from_slice(&block[1..=ASTRAL_LEN]);
    Ok(out)
}

fn detail(bus: u32) -> String {
    format!("IT8915FN · i2c-{bus}")
}

struct Astral {
    info: SourceInfo,
    pci: String,
    dev: Option<File>,
}

impl PinSource for Astral {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self) -> Result<Reading, SourceError> {
        if self.dev.is_none() {
            let bus = adapters()
                .into_iter()
                .find(|(_, pci)| *pci == self.pci)
                .map(|(bus, _)| bus)
                .ok_or_else(|| SourceError::Io(format!("no i2c adapter for {}", self.pci)))?;
            self.dev = Some(open_bus(bus).map_err(|e| SourceError::Io(format!("i2c-{bus}: {e}")))?);
            self.info.detail = detail(bus);
        }
        let dev = self.dev.as_ref().expect("opened above");
        match read_block(dev) {
            Ok(b) => Ok(Reading { pins: astral_block(&b)? }),
            Err(e) => {
                self.dev = None; // closes the fd; the bus is searched again on the next read
                Err(SourceError::Io(format!("{}: {e}", self.info.detail)))
            }
        }
    }
}

pub fn discover(gpus: &[GpuInfo]) -> Vec<Box<dyn PinSource>> {
    let mut v: Vec<Box<dyn PinSource>> = Vec::new();
    for (bus, pci) in adapters() {
        if !wanted(gpus, &pci) {
            continue;
        }
        let Ok(f) = open_bus(bus) else { continue };
        // only cards whose chip answers with a plausible pin block
        if read_block(&f).ok().and_then(|b| astral_block(&b).ok()).is_none() {
            continue;
        }
        v.push(Box::new(Astral { info: info(gpus, &pci, detail(bus)), pci, dev: Some(f) }));
    }
    v
}
