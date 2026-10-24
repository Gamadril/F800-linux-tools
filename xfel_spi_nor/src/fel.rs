use crate::{Error, Result};
use rusb::{Context, DeviceHandle, UsbContext};
use std::time::Duration;

const VID: u16 = 0x1f3a;
const PID: u16 = 0xefe8;
const TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK: usize = 128 * 1024;

#[derive(Debug, Clone)]
pub struct FelVersion {
    pub id: u32,
    pub scratchpad: u32,
}

#[derive(Debug)]
pub struct Fel {
    hdl: DeviceHandle<Context>,
    ep_in: u8,
    ep_out: u8,
    pub version: FelVersion,
}

impl Fel {
    pub fn open() -> Result<Self> {
        let context = Context::new()?;
        let hdl = context
            .open_device_with_vid_pid(VID, PID)
            .ok_or(Error::NoDevice)?;

        if hdl.kernel_driver_active(0).unwrap_or(false) {
            let _ = hdl.detach_kernel_driver(0);
        }
        hdl.claim_interface(0)?;

        let dev = hdl.device();
        let cfg = dev.active_config_descriptor()?;
        let (ep_in, ep_out) = find_bulk_endpoints(&cfg)?;

        let mut fel = Self {
            hdl,
            ep_in,
            ep_out,
            version: FelVersion {
                id: 0,
                scratchpad: 0,
            },
        };
        fel.version = fel.query_version()?;
        Ok(fel)
    }

    fn query_version(&mut self) -> Result<FelVersion> {
        self.fel_request(0x001, 0, 0)?;
        let mut raw = [0u8; 32];
        self.usb_read(&mut raw)?;
        self.read_fel_status()?;
        if &raw[..8] != b"AWUSBFEX" {
            return Err(Error::Msg("bad FEL version signature".into()));
        }
        Ok(FelVersion {
            id: u32::from_le_bytes(raw[8..12].try_into().unwrap()),
            scratchpad: u32::from_le_bytes(raw[20..24].try_into().unwrap()),
        })
    }

    pub fn read32(&mut self, addr: u32) -> Result<u32> {
        let mut v = [0u8; 4];
        self.read(addr, &mut v)?;
        Ok(u32::from_le_bytes(v))
    }

    pub fn write32(&mut self, addr: u32, val: u32) -> Result<()> {
        self.write(addr, &val.to_le_bytes())
    }

    pub fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<()> {
        check_memory_range(addr, buf.len())?;
        let mut off = 0usize;
        while off < buf.len() {
            let n = (buf.len() - off).min(65536);
            self.fel_request(0x103, addr + off as u32, n as u32)?;
            self.usb_read(&mut buf[off..off + n])?;
            self.read_fel_status()?;
            off += n;
        }
        Ok(())
    }

    pub fn write(&mut self, addr: u32, buf: &[u8]) -> Result<()> {
        check_memory_range(addr, buf.len())?;
        let mut off = 0usize;
        while off < buf.len() {
            let n = (buf.len() - off).min(65536);
            self.fel_request(0x101, addr + off as u32, n as u32)?;
            self.usb_write(&buf[off..off + n])?;
            self.read_fel_status()?;
            off += n;
        }
        Ok(())
    }

    pub fn exec(&mut self, addr: u32) -> Result<()> {
        self.fel_request(0x102, addr, 0)?;
        self.read_fel_status()
    }

    fn fel_request(&mut self, ty: u32, addr: u32, len: u32) -> Result<()> {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&ty.to_le_bytes());
        bytes[4..8].copy_from_slice(&addr.to_le_bytes());
        bytes[8..12].copy_from_slice(&len.to_le_bytes());
        self.usb_write(&bytes)
    }

    fn usb_write(&mut self, data: &[u8]) -> Result<()> {
        self.send_usb_request(0x12, data.len() as u32)?;
        self.bulk_send(data)?;
        self.read_usb_response()
    }

    fn usb_read(&mut self, data: &mut [u8]) -> Result<()> {
        self.send_usb_request(0x11, data.len() as u32)?;
        self.bulk_recv(data)?;
        self.read_usb_response()
    }

    fn send_usb_request(&mut self, ty: u16, length: u32) -> Result<()> {
        let mut bytes = [0u8; 32];
        bytes[..4].copy_from_slice(b"AWUC");
        bytes[8..12].copy_from_slice(&length.to_le_bytes());
        bytes[12..16].copy_from_slice(&0x0c00_0000u32.to_le_bytes());
        bytes[16..18].copy_from_slice(&ty.to_le_bytes());
        bytes[18..22].copy_from_slice(&length.to_le_bytes());
        self.bulk_send(&bytes)
    }

    fn read_usb_response(&mut self) -> Result<()> {
        let mut buf = [0u8; 13];
        self.bulk_recv(&mut buf)?;
        if &buf[..4] != b"AWUS" || buf[12] != 0 {
            return Err(Error::Msg("bad AWUS response".into()));
        }
        Ok(())
    }

    fn read_fel_status(&mut self) -> Result<()> {
        let mut buf = [0u8; 8];
        self.usb_read(&mut buf)?;
        validate_status(&buf)
    }

    fn bulk_send(&mut self, data: &[u8]) -> Result<()> {
        let mut off = 0;
        while off < data.len() {
            let n = (data.len() - off).min(CHUNK);
            let wrote = self
                .hdl
                .write_bulk(self.ep_out, &data[off..off + n], TIMEOUT)?;
            if wrote == 0 {
                return Err(Error::Msg("USB write made no progress".into()));
            }
            off += wrote;
        }
        Ok(())
    }

    fn bulk_recv(&mut self, data: &mut [u8]) -> Result<()> {
        let mut off = 0;
        while off < data.len() {
            let got = self.hdl.read_bulk(self.ep_in, &mut data[off..], TIMEOUT)?;
            if got == 0 {
                return Err(Error::Msg("USB read made no progress".into()));
            }
            off += got;
        }
        Ok(())
    }
}

fn find_bulk_endpoints(cfg: &rusb::ConfigDescriptor) -> Result<(u8, u8)> {
    let mut ep_in = None;
    let mut ep_out = None;
    for iface in cfg.interfaces() {
        for setting in iface.descriptors() {
            if setting.interface_number() != 0 || setting.setting_number() != 0 {
                continue;
            }
            for ep in setting.endpoint_descriptors() {
                if ep.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                if ep.direction() == rusb::Direction::In {
                    ep_in = Some(ep.address());
                } else {
                    ep_out = Some(ep.address());
                }
            }
        }
    }
    match (ep_in, ep_out) {
        (Some(i), Some(o)) => Ok((i, o)),
        _ => Err(Error::Msg("bulk endpoints not found".into())),
    }
}

fn check_memory_range(addr: u32, len: usize) -> Result<()> {
    if (addr as u64)
        .checked_add(len as u64)
        .is_none_or(|end| end > 1u64 << 32)
    {
        return Err(Error::Msg("FEL memory range exceeds 32 bits".into()));
    }
    Ok(())
}

fn validate_status(status: &[u8; 8]) -> Result<()> {
    // The first two bytes are the marker (FFFF on V853), followed by the
    // request tag. The failure state is byte 4; reserved bytes are not status.
    if status[4] != 0 {
        return Err(Error::Msg(format!(
            "FEL returned failure status {status:02x?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_status_and_overflow_are_errors() {
        assert!(validate_status(&[0; 8]).is_ok());
        assert!(validate_status(&[0xff, 0xff, 0, 0, 0, 0, 0, 0]).is_ok());
        assert!(validate_status(&[0, 0, 0, 0, 1, 0, 0, 0]).is_err());
        assert!(check_memory_range(u32::MAX, 1).is_ok());
        assert!(check_memory_range(u32::MAX, 2).is_err());
    }
}
