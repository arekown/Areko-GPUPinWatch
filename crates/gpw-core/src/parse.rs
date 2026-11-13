//! Decoders for the per-pin sources. Pure functions on byte buffers; the I/O lives in gpw-sources.
//! Protocol facts and their sources: docs/PROTOKOLLE.md.
use crate::model::{Pin, PINS};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ParseError {
    #[error("frame too short: {0} bytes")]
    Short(usize),
    #[error("implausible frame: {0}")]
    Implausible(&'static str),
    #[error("device busy")]
    Busy,
    #[error("unexpected echo {0:02x} {1:02x}")]
    Echo(u8, u8),
}

// ---------------------------------------------------------------- ASUS ROG Astral / Matrix (ITE IT8915FN)

/// 7-bit I2C address of the IT8915FN (NVAPI wants it shifted: 0x56).
pub const ASTRAL_ADDR: u8 = 0x2B;
/// Register of the 24-byte pin block.
pub const ASTRAL_REG: u8 = 0x80;
pub const ASTRAL_LEN: usize = 24;

/// 6 x (u16 BE mV, u16 BE mA); the chip lists the pins in reverse order (bytes 0..4 = pin 6).
/// Returns pins 1..6 in connector order. Plausibility: 6..16 V and <= 30 A on every pin.
pub fn astral_block(b: &[u8]) -> Result<[Pin; PINS], ParseError> {
    if b.len() < ASTRAL_LEN {
        return Err(ParseError::Short(b.len()));
    }
    let mut pins = [Pin { amps: 0.0, volts: None }; PINS];
    for i in 0..PINS {
        let o = 4 * i;
        let mv = u16::from_be_bytes([b[o], b[o + 1]]) as f32;
        let ma = u16::from_be_bytes([b[o + 2], b[o + 3]]) as f32;
        let (v, a) = (mv / 1000.0, ma / 1000.0);
        if !(6.0..=16.0).contains(&v) || a > 30.0 {
            return Err(ParseError::Implausible("astral pin out of range"));
        }
        pins[PINS - 1 - i] = Pin { amps: a, volts: Some(v) };
    }
    Ok(pins)
}

// ---------------------------------------------------------------- Thermal Grizzly WireView Pro II

/// USB CDC-ACM id; 115200 8N1.
pub const WIREVIEW_VID: u16 = 0x0483;
pub const WIREVIEW_PID: u16 = 0x5740;
/// The only commands this program ever sends (all read-only). Everything else is refused in gpw-sources.
pub const WV_CMD_IDENTITY: u8 = 0x01; // -> 3 bytes [vendor 0xEF, product 0x05 Pro II / 0x06 Noctua, fw]
pub const WV_CMD_UID: u8 = 0x02; // -> 12 bytes
pub const WV_CMD_LIVE: u8 = 0x04; // -> 100 bytes
pub const WV_IDENTITY_LEN: usize = 3;
pub const WV_UID_LEN: usize = 12;
pub const WV_LIVE_LEN: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct WireViewLive {
    pub pins: [Pin; PINS],
    /// TS_IN, TS_OUT, external 1, external 2 in °C (None = probe not connected).
    pub temps: [Option<f32>; 4],
    /// Bit mask: 0 chip overtemp, 1 probe overtemp, 2 total overcurrent, 3 wire overcurrent, 4 overpower, 5 imbalance.
    pub faults_active: u16,
    /// Cable capability in W reported by the sense pins (600/450/300/150).
    pub cable_w: u16,
}

/// True for a WireView identity the program supports (Pro II 0x05, Noctua Edition 0x06).
pub fn wireview_supported(identity: &[u8]) -> bool {
    identity.len() >= 2 && identity[0] == 0xEF && (identity[1] == 0x05 || identity[1] == 0x06)
}

/// Live sensor frame (command 0x04), little endian, checked like the reference implementations.
pub fn wireview_live(b: &[u8]) -> Result<WireViewLive, ParseError> {
    if b.len() < WV_LIVE_LEN {
        return Err(ParseError::Short(b.len()));
    }
    let i16le = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    let u16le = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if b[11] != 0 || b[95] != 0 {
        return Err(ParseError::Implausible("wireview padding not zero"));
    }
    if b[10] > 100 || u16le(8) > 6000 {
        return Err(ParseError::Implausible("wireview fan/vdd out of range"));
    }
    let cable_w = match b[94] {
        0 => 600,
        1 => 450,
        2 => 300,
        3 => 150,
        _ => return Err(ParseError::Implausible("wireview cable capability")),
    };
    let mut pins = [Pin { amps: 0.0, volts: None }; PINS];
    for (n, pin) in pins.iter_mut().enumerate() {
        let mv = i16le(12 + 12 * n);
        let ma = u32le(16 + 12 * n);
        if !(-1000..=20000).contains(&mv) || ma > 60_000 {
            return Err(ParseError::Implausible("wireview pin out of range"));
        }
        *pin = Pin { amps: ma as f32 / 1000.0, volts: Some(mv as f32 / 1000.0) };
    }
    let mut temps = [None; 4];
    for (k, t) in temps.iter_mut().enumerate() {
        let raw = i16le(2 * k);
        *t = if raw == -1000 { None } else { Some(raw as f32 / 10.0) };
    }
    Ok(WireViewLive { pins, temps, faults_active: u16le(96), cable_w })
}

// ---------------------------------------------------------------- MSI MPG Ai1300TS / Ai1600TS (HID)

pub const MSI_VID: u16 = 0x0DB0;
pub const MSI_PID_AI1300TS: u16 = 0xAA6F;
pub const MSI_PID_AI1600TS: u16 = 0x808C;
/// HID report size without report id (Linux hidraw writes 0x00 + 64 bytes).
pub const MSI_REPORT: usize = 64;
/// Handshake sent once per connection.
pub const MSI_HANDSHAKE: [u8; 2] = [0xFA, 0x51];
/// Read opcode; the only registers this program reads.
pub const MSI_READ: u8 = 0x51;
pub const MSI_REG_PINS: u8 = 0xE0;
pub const MSI_REG_STATUS: u8 = 0xC1;
pub const MSI_REG_MODEL: u8 = 0x11;
pub const MSI_REGS_ALLOWED: [u8; 8] = [0x10, 0x11, 0x12, 0x13, 0xC0, 0xC1, 0xE0, 0xE1];

/// PMBus LINEAR11 as MSI uses it: 5-bit signed exponent, 11-bit unsigned mantissa.
pub fn linear11(raw: u16) -> f32 {
    let exp = ((raw >> 11) as i16) << 11 >> 11; // sign-extend 5 bits
    let mant = (raw & 0x07FF) as f32;
    mant * 2f32.powi(exp as i32)
}

/// Request for one register: [0x51, reg, 0...] (64 bytes, report id added by the transport).
pub fn msi_request(reg: u8) -> [u8; MSI_REPORT] {
    let mut r = [0u8; MSI_REPORT];
    r[0] = MSI_READ;
    r[1] = reg;
    r
}

/// Checks the echo of a reply and returns its data part (after "51 reg").
pub fn msi_data(reply: &[u8], reg: u8) -> Result<&[u8], ParseError> {
    if reply.len() < 2 {
        return Err(ParseError::Short(reply.len()));
    }
    if reply[0] == 0xFE && reply[1..].iter().all(|&x| x == 0) {
        return Err(ParseError::Busy);
    }
    if reply[0] != MSI_READ || reply[1] != reg {
        return Err(ParseError::Echo(reply[0], reply[1]));
    }
    Ok(&reply[2..])
}

#[derive(Debug, Clone, PartialEq)]
pub struct MsiPins {
    /// Two 12V-2x6 connectors x pins 1..6 in A (unused connector = all 0).
    pub connectors: [[f32; PINS]; 2],
    /// PSU output power in W, when present in the frame.
    pub power_w: Option<f32>,
}

/// Register 0xE0 data: u16 LE LINEAR11; offset 6 + 12*connector + 2*pin; output power at 30.
pub fn msi_pins(data: &[u8]) -> Result<MsiPins, ParseError> {
    if data.len() < 30 {
        return Err(ParseError::Short(data.len()));
    }
    let at = |o: usize| linear11(u16::from_le_bytes([data[o], data[o + 1]]));
    let mut connectors = [[0f32; PINS]; 2];
    for (c, conn) in connectors.iter_mut().enumerate() {
        for (p, a) in conn.iter_mut().enumerate() {
            *a = at(6 + 12 * c + 2 * p);
            if *a > 60.0 {
                return Err(ParseError::Implausible("msi pin out of range"));
            }
        }
    }
    let power_w = if data.len() >= 32 { Some(at(30)) } else { None };
    Ok(MsiPins { connectors, power_w })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astral_block_reverses_pins_and_scales() {
        // same bytes as read from areko-pc-1 (1043:89E3), pin 6 first
        let b = [
            0x2f, 0x00, 0x04, 0x38, 0x2e, 0xf8, 0x03, 0xe8, 0x2e, 0xf8, 0x03, 0xfc, 0x2f, 0x00, 0x04, 0x38, 0x2f, 0x00,
            0x04, 0x38, 0x2f, 0x00, 0x24, 0x10,
        ];
        let p = astral_block(&b).unwrap();
        assert_eq!(p[0].amps, 9.232); // last block = pin 1
        assert_eq!(p[5].amps, 1.08); // first block = pin 6
        assert_eq!(p[0].volts, Some(12.032));
        let mut bad = b;
        bad[0] = 0; // 0 V on pin 6
        assert!(astral_block(&bad).is_err());
    }

    fn wv_frame(ma: [u32; 6]) -> [u8; WV_LIVE_LEN] {
        let mut b = [0u8; WV_LIVE_LEN];
        b[0..2].copy_from_slice(&(352i16).to_le_bytes());
        b[4..6].copy_from_slice(&(-1000i16).to_le_bytes());
        b[8..10].copy_from_slice(&3300u16.to_le_bytes());
        b[10] = 40;
        for (n, a) in ma.iter().enumerate() {
            b[12 + 12 * n..14 + 12 * n].copy_from_slice(&12050i16.to_le_bytes());
            b[16 + 12 * n..20 + 12 * n].copy_from_slice(&a.to_le_bytes());
        }
        b[96..98].copy_from_slice(&0b1000u16.to_le_bytes());
        b
    }

    #[test]
    fn wireview_live_decodes_and_rejects_desync() {
        let b = wv_frame([7100, 7200, 8650, 7000, 6900, 7050]);
        let w = wireview_live(&b).unwrap();
        assert_eq!(w.pins[2].amps, 8.65);
        assert_eq!(w.pins[0].volts, Some(12.05));
        assert_eq!(w.temps[0], Some(35.2));
        assert_eq!(w.temps[2], None);
        assert_eq!(w.cable_w, 600);
        assert_eq!(w.faults_active, 0b1000);
        let mut shifted = [0u8; WV_LIVE_LEN];
        shifted[1..].copy_from_slice(&b[..WV_LIVE_LEN - 1]); // one byte out of step
        assert!(wireview_live(&shifted).is_err());
        assert!(wireview_supported(&[0xEF, 0x06, 3]) && !wireview_supported(&[0xEF, 0x07, 1]));
    }

    #[test]
    fn linear11_and_msi_frame() {
        // exponent -4 (0b11100), mantissa 2 -> 0.125 A; mantissa 136 -> 8.5 A
        let enc = |m: u16| (0b11100u16 << 11) | m;
        assert_eq!(linear11(enc(2)), 0.125);
        assert_eq!(linear11(enc(136)), 8.5);
        let mut reply = [0u8; MSI_REPORT];
        reply[0] = 0x51;
        reply[1] = 0xE0;
        for p in 0..6 {
            let o = 2 + 18 + 2 * p; // connector 2
            reply[o..o + 2].copy_from_slice(&enc(if p == 3 { 147 } else { 96 }).to_le_bytes());
        }
        reply[2 + 30..2 + 32].copy_from_slice(&((0b00000u16 << 11) | 575).to_le_bytes());
        let d = msi_data(&reply, 0xE0).unwrap();
        let m = msi_pins(d).unwrap();
        assert_eq!(m.connectors[0], [0.0; 6]);
        assert_eq!(m.connectors[1][3], 9.1875);
        assert_eq!(m.power_w, Some(575.0));
        let mut busy = [0u8; MSI_REPORT];
        busy[0] = 0xFE;
        assert_eq!(msi_data(&busy, 0xE0), Err(ParseError::Busy));
        assert!(msi_data(&reply, 0xC1).is_err());
    }
}
