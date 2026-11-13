//! Windows: nvapi64.dll, NvAPI_I2CReadEx on the GPU's I2C port 1. Never I2CWriteEx.
use super::{info, wanted};
use crate::{PinSource, Reading, SourceError};
use gpw_core::model::{norm_pci, GpuInfo, SourceInfo};
use gpw_core::parse::{astral_block, ASTRAL_ADDR, ASTRAL_LEN, ASTRAL_REG};
use libloading::Library;
use std::ffi::c_void;
use std::sync::Arc;

// nvapi_QueryInterface ids (docs/PROTOKOLLE.md section 1)
const ID_INITIALIZE: u32 = 0x0150_E828;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC_921F;
const ID_GET_BUS_ID: u32 = 0x1BE0_B8E5;
const ID_GET_BUS_SLOT_ID: u32 = 0x2A0A_350F;
const ID_I2C_READ_EX: u32 = 0x4D7B_0709;

const NVAPI_OK: i32 = 0;
const NVAPI_MAX_PHYSICAL_GPUS: usize = 64;
const NV_I2C_INFO_V3_VERSION: u32 = 0x0003_0040; // version 3, size 64

type NvHandle = *mut c_void;
type QueryInterfaceFn = unsafe extern "C" fn(u32) -> *mut c_void;
type InitializeFn = unsafe extern "C" fn() -> i32;
type EnumGpusFn = unsafe extern "C" fn(*mut [NvHandle; NVAPI_MAX_PHYSICAL_GPUS], *mut u32) -> i32;
type GetU32Fn = unsafe extern "C" fn(NvHandle, *mut u32) -> i32;
type I2cReadExFn = unsafe extern "C" fn(NvHandle, *mut NvI2cInfoV3, *mut u32) -> i32;

/// NV_I2C_INFO_V3, x64 layout (64 bytes); offsets in the comments.
#[repr(C)]
struct NvI2cInfoV3 {
    version: u32,           // 0
    display_mask: u32,      // 4
    is_ddc_port: u8,        // 8
    i2c_dev_address: u8,    // 9
    reg_address: *mut u8,   // 16
    reg_addr_size: u32,     // 24
    data: *mut u8,          // 32
    cb_size: u32,           // 40
    i2c_speed: u32,         // 44
    i2c_speed_khz: u32,     // 48
    port_id: u8,            // 52
    is_port_id_set: u32,    // 56
}
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<NvI2cInfoV3>() == 64);

struct Nvapi {
    _lib: Library,
    read: I2cReadExFn,
}

/// GPUs as (handle, normalised PCI address) plus the shared API.
fn load() -> Option<(Arc<Nvapi>, Vec<(usize, String)>)> {
    // SAFETY: loading the NVIDIA driver library and resolving its documented entry point.
    let lib = unsafe { Library::new("nvapi64.dll") }.ok()?;
    let qi: QueryInterfaceFn = unsafe { *lib.get::<QueryInterfaceFn>(b"nvapi_QueryInterface\0").ok()? };
    let get = |id: u32| {
        // SAFETY: nvapi_QueryInterface returns a function pointer or null.
        let p = unsafe { qi(id) };
        (!p.is_null()).then_some(p)
    };
    // SAFETY: each pointer is transmuted to the signature the id stands for.
    let init: InitializeFn = unsafe { std::mem::transmute(get(ID_INITIALIZE)?) };
    let enum_gpus: EnumGpusFn = unsafe { std::mem::transmute(get(ID_ENUM_PHYSICAL_GPUS)?) };
    let bus_id: GetU32Fn = unsafe { std::mem::transmute(get(ID_GET_BUS_ID)?) };
    let slot_id: GetU32Fn = unsafe { std::mem::transmute(get(ID_GET_BUS_SLOT_ID)?) };
    let read: I2cReadExFn = unsafe { std::mem::transmute(get(ID_I2C_READ_EX)?) };
    if unsafe { init() } != NVAPI_OK {
        return None;
    }
    let mut handles = [std::ptr::null_mut(); NVAPI_MAX_PHYSICAL_GPUS];
    let mut count = 0u32;
    if unsafe { enum_gpus(&mut handles, &mut count) } != NVAPI_OK {
        return None;
    }
    let mut gpus = Vec::new();
    for &h in handles.iter().take(count as usize) {
        let (mut bus, mut slot) = (0u32, 0u32);
        if unsafe { bus_id(h, &mut bus) } != NVAPI_OK || unsafe { slot_id(h, &mut slot) } != NVAPI_OK {
            continue;
        }
        gpus.push((h as usize, norm_pci(&format!("0000:{bus:02x}:{slot:02x}.0"))));
    }
    Some((Arc::new(Nvapi { _lib: lib, read }), gpus))
}

fn read_block(api: &Nvapi, gpu: usize) -> Result<[u8; ASTRAL_LEN], SourceError> {
    let mut reg = [ASTRAL_REG];
    let mut data = [0u8; ASTRAL_LEN];
    let mut info = NvI2cInfoV3 {
        version: NV_I2C_INFO_V3_VERSION,
        display_mask: 0,
        is_ddc_port: 0,
        i2c_dev_address: ASTRAL_ADDR << 1, // NVAPI takes the 8-bit form: 0x56
        reg_address: reg.as_mut_ptr(),
        reg_addr_size: 1,
        data: data.as_mut_ptr(),
        cb_size: ASTRAL_LEN as u32,
        i2c_speed: 0xFFFF,
        i2c_speed_khz: 4, // 100 kHz
        port_id: 1,
        is_port_id_set: 1,
    };
    let mut unknown = 0u32;
    // SAFETY: `info` points to live buffers of the declared sizes for the duration of the call.
    let status = unsafe { (api.read)(gpu as NvHandle, &mut info, &mut unknown) };
    if status != NVAPI_OK {
        return Err(SourceError::Io(format!("NvAPI_I2CReadEx status {status}")));
    }
    Ok(data)
}

struct Astral {
    info: SourceInfo,
    api: Arc<Nvapi>,
    gpu: usize,
}

impl PinSource for Astral {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self) -> Result<Reading, SourceError> {
        let b = read_block(&self.api, self.gpu)?;
        Ok(Reading { pins: astral_block(&b)? })
    }
}

pub fn discover(gpus: &[GpuInfo]) -> Vec<Box<dyn PinSource>> {
    let mut v: Vec<Box<dyn PinSource>> = Vec::new();
    let Some((api, handles)) = load() else { return v };
    for (gpu, pci) in handles {
        if !wanted(gpus, &pci) {
            continue;
        }
        // only cards whose chip answers with a plausible pin block
        if read_block(&api, gpu).ok().and_then(|b| astral_block(&b).ok()).is_none() {
            continue;
        }
        let detail = "IT8915FN · NVAPI".to_string();
        v.push(Box::new(Astral { info: info(gpus, &pci, detail), api: api.clone(), gpu }));
    }
    v
}
