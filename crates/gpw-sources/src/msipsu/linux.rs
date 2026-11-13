//! Linux: hidraw. Writes 0x00 (no report id) + 64 bytes, reads 64-byte input reports.
use super::supported;
use gpw_core::parse::MSI_REPORT;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

pub struct Found {
    pub path: String,
    pub pid: u16,
    /// Short name for the detail line, e.g. "hidraw2".
    pub name: String,
}

/// Supported PSUs by `HID_ID=0003:00000DB0:0000XXXX` in /sys/class/hidraw/*/device/uevent.
pub fn find() -> Vec<Found> {
    let mut v = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/hidraw") else { return v };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(uevent) = fs::read_to_string(e.path().join("device/uevent")) else { continue };
        let Some(id) = uevent.lines().find_map(|l| l.strip_prefix("HID_ID=")) else { continue };
        let mut parts = id.split(':').skip(1).map(|p| u32::from_str_radix(p.trim(), 16).ok());
        let (Some(Some(vid)), Some(Some(pid))) = (parts.next(), parts.next()) else { continue };
        if supported(vid as u16, pid as u16) {
            v.push(Found { path: format!("/dev/{name}"), pid: pid as u16, name });
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub struct Hid {
    file: File,
}

/// No cross-process lock on Linux (the MSI tools that share the Windows mutex do not run here).
pub struct Lock;

impl Hid {
    pub fn open(path: &str) -> io::Result<Hid> {
        let file = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        Ok(Hid { file })
    }

    pub fn lock(&self) -> io::Result<Lock> {
        Ok(Lock)
    }

    /// Discards every input report already queued.
    pub fn drain(&mut self) {
        let mut buf = [0u8; MSI_REPORT];
        while matches!(self.file.read(&mut buf), Ok(n) if n > 0) {}
    }

    pub fn write(&mut self, report: &[u8; MSI_REPORT]) -> io::Result<()> {
        let mut out = [0u8; MSI_REPORT + 1];
        out[1..].copy_from_slice(report);
        let n = self.file.write(&out)?;
        if n != out.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short hid write {n}")));
        }
        Ok(())
    }

    /// One input report, or None when nothing arrived within `timeout`.
    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<[u8; MSI_REPORT]>> {
        let mut pfd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
        // SAFETY: one pollfd that lives for the duration of the call.
        let r = unsafe { libc::poll(&mut pfd, 1, ms) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        if r == 0 {
            return Ok(None);
        }
        let mut buf = [0u8; MSI_REPORT];
        match self.file.read(&mut buf) {
            Ok(n) if n >= 2 => Ok(Some(buf)),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}
