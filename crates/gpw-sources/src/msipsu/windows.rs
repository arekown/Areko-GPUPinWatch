//! Windows: HID through SetupAPI / hid.dll / overlapped ReadFile+WriteFile (no admin needed).
//! MSI Center, Afterburner and HWiNFO share `Global\MSI_PSU_Mutex`; it is held around every request.
use super::supported;
use gpw_core::parse::MSI_REPORT;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::Duration;
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FlushQueue, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps,
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenMutexW, ReleaseMutex, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};

const USAGE_PAGE_VENDOR: u16 = 0xFF00;
const MUTEX_NAME: &str = "Global\\MSI_PSU_Mutex";
const MUTEX_WAIT_MS: u32 = 500;
const WRITE_WAIT_MS: u32 = 500;

pub struct Found {
    pub path: String,
    pub pid: u16,
    /// Short name for the detail line.
    pub name: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_file(path: &str, access: u32, flags: u32) -> io::Result<HANDLE> {
    let p = wide(path);
    // SAFETY: NUL-terminated path; all other arguments are plain values or null.
    let h = unsafe {
        CreateFileW(p.as_ptr(), access, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, flags, null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(h)
}

/// (vendor id, product id, usage page, input report length, output report length) of an open HID handle.
fn describe(h: HANDLE) -> Option<(u16, u16, u16, usize, usize)> {
    // SAFETY: plain hid.dll queries on a valid handle into zero-initialised out structs.
    unsafe {
        let mut attr: HIDD_ATTRIBUTES = zeroed();
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        if !HidD_GetAttributes(h, &mut attr) {
            return None;
        }
        let mut pre = 0;
        if !HidD_GetPreparsedData(h, &mut pre) {
            return None;
        }
        let mut caps: HIDP_CAPS = zeroed();
        let st = HidP_GetCaps(pre, &mut caps);
        HidD_FreePreparsedData(pre);
        if st != HIDP_STATUS_SUCCESS {
            return None;
        }
        Some((
            attr.VendorID,
            attr.ProductID,
            caps.UsagePage,
            caps.InputReportByteLength as usize,
            caps.OutputReportByteLength as usize,
        ))
    }
}

/// Every present HID interface of a supported PSU on the vendor usage page 0xFF00.
pub fn find() -> Vec<Found> {
    let mut v = Vec::new();
    // SAFETY: SetupAPI enumeration with buffers sized as the API reports; the list is destroyed at the end.
    unsafe {
        let mut guid: GUID = zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(&guid, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set == -1 {
            return v;
        }
        let mut index = 0u32;
        loop {
            let mut ifd: SP_DEVICE_INTERFACE_DATA = zeroed();
            ifd.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, null(), &guid, index, &mut ifd) == 0 {
                break;
            }
            index += 1;
            let mut need = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &ifd, null_mut(), 0, &mut need, null_mut());
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u32; (need as usize).div_ceil(4)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &ifd, detail, need, null_mut(), null_mut()) == 0 {
                continue;
            }
            let p = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            // access 0: attributes and caps only, works while other programs hold the device
            let Ok(h) = create_file(&path, 0, 0) else { continue };
            let d = describe(h);
            CloseHandle(h);
            if let Some((vid, pid, page, _, _)) = d {
                if supported(vid, pid) && page == USAGE_PAGE_VENDOR {
                    v.push(Found { path, pid, name: "HID".to_string() });
                }
            }
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    v
}

pub struct Hid {
    handle: HANDLE,
    event: HANDLE,
    in_len: usize,
    out_len: usize,
}

// SAFETY: the handles are only used through &mut self, behind the PSU's Mutex.
unsafe impl Send for Hid {}

impl Drop for Hid {
    fn drop(&mut self) {
        // SAFETY: both handles are owned by this struct.
        unsafe {
            CancelIo(self.handle);
            CloseHandle(self.handle);
            CloseHandle(self.event);
        }
    }
}

/// Held `Global\MSI_PSU_Mutex`; released and closed on drop.
pub struct Lock(HANDLE);

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: the mutex is owned by this thread since the successful wait.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

impl Hid {
    pub fn open(path: &str) -> io::Result<Hid> {
        let handle = create_file(path, GENERIC_READ | GENERIC_WRITE, FILE_FLAG_OVERLAPPED)?;
        let Some((_, _, _, in_len, out_len)) = describe(handle) else {
            // SAFETY: handle was just opened here
            unsafe { CloseHandle(handle) };
            return Err(io::Error::other("hid caps not readable"));
        };
        // SAFETY: unnamed manual-reset event for the overlapped transfers.
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            let e = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(e);
        }
        Ok(Hid { handle, event, in_len: in_len.max(MSI_REPORT + 1), out_len: out_len.max(MSI_REPORT + 1) })
    }

    /// Takes `Global\MSI_PSU_Mutex` (opened with SYNCHRONIZE, created when missing).
    pub fn lock(&self) -> io::Result<Lock> {
        let name = wide(MUTEX_NAME);
        // SAFETY: NUL-terminated name; the handle is closed by `Lock` or below.
        unsafe {
            let mut m = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
            if m.is_null() {
                m = CreateMutexW(null(), 0, name.as_ptr());
            }
            if m.is_null() {
                return Err(io::Error::last_os_error());
            }
            match WaitForSingleObject(m, MUTEX_WAIT_MS) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Lock(m)),
                _ => {
                    CloseHandle(m);
                    Err(io::Error::new(io::ErrorKind::TimedOut, "MSI_PSU_Mutex busy"))
                }
            }
        }
    }

    /// Discards every input report already queued.
    pub fn drain(&mut self) {
        // SAFETY: valid HID handle.
        unsafe { HidD_FlushQueue(self.handle) };
    }

    /// Waits for the overlapped transfer started on `ov`; cancels it after `ms`.
    /// Returns the bytes transferred, or None on timeout.
    unsafe fn finish(&self, started: i32, ov: &mut OVERLAPPED, ms: u32) -> io::Result<Option<u32>> {
        if started == 0 && GetLastError() != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        let mut n = 0u32;
        if WaitForSingleObject(self.event, ms) == WAIT_TIMEOUT {
            CancelIo(self.handle);
            GetOverlappedResult(self.handle, ov, &mut n, 1); // wait until the cancel completed
            return Ok(None);
        }
        if GetOverlappedResult(self.handle, ov, &mut n, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(n))
    }

    pub fn write(&mut self, report: &[u8; MSI_REPORT]) -> io::Result<()> {
        let mut buf = vec![0u8; self.out_len]; // byte 0 = report id 0
        buf[1..=MSI_REPORT].copy_from_slice(report);
        // SAFETY: `buf` and `ov` live until the transfer finished or was cancelled in `finish`.
        unsafe {
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = self.event;
            let started = WriteFile(self.handle, buf.as_ptr(), buf.len() as u32, null_mut(), &mut ov);
            match self.finish(started, &mut ov, WRITE_WAIT_MS)? {
                Some(_) => Ok(()),
                None => Err(io::Error::new(io::ErrorKind::TimedOut, "hid write timed out")),
            }
        }
    }

    /// One input report, or None when nothing arrived within `timeout`.
    pub fn read(&mut self, timeout: Duration) -> io::Result<Option<[u8; MSI_REPORT]>> {
        let mut buf = vec![0u8; self.in_len];
        let ms = timeout.as_millis().clamp(1, u32::MAX as u128) as u32;
        // SAFETY: `buf` and `ov` live until the transfer finished or was cancelled in `finish`.
        let n = unsafe {
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = self.event;
            let started = ReadFile(self.handle, buf.as_mut_ptr(), buf.len() as u32, null_mut(), &mut ov);
            self.finish(started, &mut ov, ms)?
        };
        match n {
            Some(n) if n as usize > MSI_REPORT => {
                let mut r = [0u8; MSI_REPORT];
                r.copy_from_slice(&buf[1..=MSI_REPORT]); // skip report id 0
                Ok(Some(r))
            }
            _ => Ok(None),
        }
    }
}
