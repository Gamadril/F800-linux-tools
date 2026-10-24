//! SPI NOR over FEL. The V853 backend supplies only the SRAM payload and buffers.

use crate::fel::Fel;
use crate::progress::Progress;
use crate::v853;
use crate::{Error, Result};

#[repr(u8)]
enum SpiCmd {
    End = 0,
    Init = 1,
    Select = 2,
    Deselect = 3,
    TxBuf = 5,
    RxBuf = 6,
}

#[derive(Clone, Debug)]
struct SpinorInfo {
    name: String,
    id: u32,
    capacity: u64,
    page_size: usize,
    address_length: usize,
    erase: Vec<(u32, u8)>,
    source: &'static str,
    sfdp_capacity: Option<u64>,
}

impl SpinorInfo {
    fn check_range(&self, addr: u64, len: u64) -> Result<()> {
        let end = addr
            .checked_add(len)
            .ok_or_else(|| error("flash range overflows"))?;
        let address_limit = 1u64 << (self.address_length * 8);
        if end > self.capacity || end > address_limit {
            return Err(error(format!(
                "flash range {addr:#x}..{end:#x} exceeds capacity {:#x}",
                self.capacity
            )));
        }
        Ok(())
    }

    fn check_erase(&self, addr: u64, len: u64) -> Result<()> {
        self.check_range(addr, len)?;
        let unit = self
            .erase
            .iter()
            .map(|e| e.0 as u64)
            .min()
            .ok_or_else(|| error("flash has no supported erase opcode"))?;
        if !addr.is_multiple_of(unit) || !len.is_multiple_of(unit) {
            return Err(error(format!("erase/write range must be aligned to {unit} bytes; boundary bytes are never erased implicitly")));
        }
        Ok(())
    }
}

fn error(message: impl Into<String>) -> Error {
    Error::Msg(message.into())
}

// This small transport boundary lets tests inject failures without a USB device.
trait SpiTransport {
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<()>;
    fn read(&mut self, addr: u32, data: &mut [u8]) -> Result<()>;
    fn run(&mut self, commands: &[u8]) -> Result<()>;
}
impl SpiTransport for Fel {
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        Fel::write(self, addr, data)
    }
    fn read(&mut self, addr: u32, data: &mut [u8]) -> Result<()> {
        Fel::read(self, addr, data)
    }
    fn run(&mut self, commands: &[u8]) -> Result<()> {
        v853::spi_run(self, commands)
    }
}

pub struct SpinorFlash {
    fel: Fel,
}

#[derive(Clone, Debug)]
pub struct SpinorDetected {
    pub name: String,
    pub capacity: u64,
    pub jedec_id: u32,
    pub source: &'static str,
    pub sfdp_capacity: Option<u64>,
}

impl SpinorFlash {
    pub fn open() -> Result<Self> {
        let fel = Fel::open()?;
        if !v853::is_v853(&fel) {
            return Err(Error::NotV853(fel.version.id));
        }
        Ok(Self { fel })
    }
    pub fn version_id(&self) -> u32 {
        self.fel.version.id
    }
    pub fn reset(&mut self) -> Result<()> {
        v853::reset(&mut self.fel)
    }

    fn init(&mut self) -> Result<SpinorInfo> {
        v853::upload_spi_payload(&mut self.fel)?;
        self.fel.run(&[SpiCmd::Init as u8, SpiCmd::End as u8])?;
        let info = detect(&mut self.fel)?;
        // Read/detect never unlock the chip, clear protection bits, or reset it.
        if info.address_length == 4 {
            spi_xfer(&mut self.fel, &[0xb7], None)?;
        }
        Ok(info)
    }

    pub fn detect(&mut self) -> Result<SpinorDetected> {
        let info = self.init()?;
        Ok(SpinorDetected {
            name: info.name,
            capacity: info.capacity,
            jedec_id: info.id,
            source: info.source,
            sfdp_capacity: info.sfdp_capacity,
        })
    }

    pub fn validate_erase_range(&mut self, addr: u64, len: u64) -> Result<()> {
        self.init()?.check_erase(addr, len)
    }
    pub fn validate_read_range(&mut self, addr: u64, len: u64) -> Result<()> {
        self.init()?.check_range(addr, len)
    }

    pub fn read(&mut self, addr: u64, buf: &mut [u8]) -> Result<()> {
        let info = self.init()?;
        info.check_range(addr, buf.len() as u64)?;
        read_data(&mut self.fel, &info, addr, buf)
    }

    /// Only whole erase units are accepted. Always verifies programmed bytes.
    pub fn write(&mut self, addr: u64, data: &[u8]) -> Result<()> {
        let info = self.init()?;
        info.check_erase(addr, data.len() as u64)?;
        if data.is_empty() {
            return Ok(());
        }
        check_unprotected(&mut self.fel)?;
        erase_data(&mut self.fel, &info, addr, data.len() as u64)?;
        let mut progress = Progress::start(data.len() as u64);
        let mut offset = 0;
        while offset < data.len() {
            // Never cross a NOR page, even if a future caller starts mid-page.
            let address = addr + offset as u64;
            let n = (info.page_size - address as usize % info.page_size).min(data.len() - offset);
            let mut tx = address_command(0x02, &info, address);
            tx.extend_from_slice(&data[offset..offset + n]);
            spi_xfer(&mut self.fel, &[0x06], None)?;
            spi_xfer(&mut self.fel, &tx, None)?;
            wait_ready(&mut self.fel)?;
            offset += n;
            if let Some(p) = progress.as_mut() {
                p.update(n as u64);
            }
        }
        if let Some(p) = progress {
            p.stop();
        }
        let mut actual = vec![0; data.len()];
        read_data(&mut self.fel, &info, addr, &mut actual)?;
        verify_bytes(addr, data, &actual)
    }

    pub fn erase(&mut self, addr: u64, len: u64) -> Result<()> {
        let info = self.init()?;
        info.check_erase(addr, len)?;
        if len == 0 {
            return Ok(());
        }
        check_unprotected(&mut self.fel)?;
        erase_data(&mut self.fel, &info, addr, len)
    }

    /// Raw SFDP bytes for diagnosis; no flash register or array writes.
    pub fn read_sfdp(&mut self, addr: u32, data: &mut [u8]) -> Result<()> {
        v853::upload_spi_payload(&mut self.fel)?;
        self.fel.run(&[SpiCmd::Init as u8, SpiCmd::End as u8])?;
        sfdp_read(&mut self.fel, addr, data)
    }
}

fn verify_bytes(addr: u64, expected: &[u8], actual: &[u8]) -> Result<()> {
    if expected.len() != actual.len() {
        return Err(error("verification length mismatch"));
    }
    if let Some(i) = expected.iter().zip(actual).position(|(a, b)| a != b) {
        return Err(error(format!(
            "read-back verification failed at {:#x}: expected {:02x}, got {:02x}",
            addr + i as u64,
            expected[i],
            actual[i]
        )));
    }
    Ok(())
}

fn address_command(opcode: u8, info: &SpinorInfo, addr: u64) -> Vec<u8> {
    let mut tx = vec![opcode];
    tx.extend_from_slice(&(addr as u32).to_be_bytes()[4 - info.address_length..]);
    tx
}

fn push_buf(c: &mut Vec<u8>, op: SpiCmd, len: usize) {
    c.push(op as u8);
    c.extend_from_slice(&v853::SWAP_BUF.to_le_bytes());
    c.extend_from_slice(&(len as u32).to_le_bytes());
}

fn spi_xfer(t: &mut impl SpiTransport, tx: &[u8], rx: Option<&mut [u8]>) -> Result<()> {
    let rxlen = rx.as_ref().map_or(0, |r| r.len());
    if tx.len() > v853::SWAP_LEN as usize || rxlen > v853::SWAP_LEN as usize {
        return Err(error("SPI transfer exceeds SRAM buffer"));
    }
    let mut c = vec![SpiCmd::Select as u8];
    if !tx.is_empty() {
        t.write(v853::SWAP_BUF, tx)?;
        push_buf(&mut c, SpiCmd::TxBuf, tx.len());
    }
    if rxlen > 0 {
        push_buf(&mut c, SpiCmd::RxBuf, rxlen);
    }
    c.extend_from_slice(&[SpiCmd::Deselect as u8, SpiCmd::End as u8]);
    t.run(&c)?;
    if let Some(buf) = rx {
        t.read(v853::SWAP_BUF, buf)?;
    }
    Ok(())
}

fn read_data(
    t: &mut impl SpiTransport,
    info: &SpinorInfo,
    addr: u64,
    data: &mut [u8],
) -> Result<()> {
    let mut progress = Progress::start(data.len() as u64);
    for (index, chunk) in data.chunks_mut(v853::SWAP_LEN as usize).enumerate() {
        let address = addr + (index * v853::SWAP_LEN as usize) as u64;
        spi_xfer(t, &address_command(0x03, info, address), Some(chunk))?;
        if let Some(p) = progress.as_mut() {
            p.update(chunk.len() as u64);
        }
    }
    if let Some(p) = progress {
        p.stop();
    }
    Ok(())
}

fn wait_ready(t: &mut impl SpiTransport) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let mut status = [0];
        spi_xfer(t, &[0x05], Some(&mut status))?;
        if status[0] & 1 == 0 {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(error("SPI NOR busy timeout"));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn check_unprotected(t: &mut impl SpiTransport) -> Result<()> {
    let mut status = [0];
    spi_xfer(t, &[0x05], Some(&mut status))?;
    if status[0] & 0x3c != 0 {
        return Err(error(
            "NOR is protected; refusing to change protection registers automatically",
        ));
    }
    Ok(())
}
fn erase_data(
    t: &mut impl SpiTransport,
    info: &SpinorInfo,
    mut addr: u64,
    mut len: u64,
) -> Result<()> {
    let mut progress = Progress::start(len);
    while len > 0 {
        let &(size, opcode) = info
            .erase
            .iter()
            .filter(|(size, _)| addr.is_multiple_of(*size as u64) && len >= *size as u64)
            .max_by_key(|(size, _)| *size)
            .ok_or_else(|| error("unsupported erase alignment"))?;
        spi_xfer(t, &[0x06], None)?;
        spi_xfer(t, &address_command(opcode, info, addr), None)?;
        wait_ready(t)?;
        addr += size as u64;
        len -= size as u64;
        if let Some(p) = progress.as_mut() {
            p.update(size as u64);
        }
    }
    if let Some(p) = progress {
        p.stop();
    }
    Ok(())
}

fn sfdp_read(t: &mut impl SpiTransport, addr: u32, data: &mut [u8]) -> Result<()> {
    if addr as u64 + data.len() as u64 > 0x1000000 {
        return Err(error("SFDP range exceeds 24 bits"));
    }
    let tx = [0x5a, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8, 0];
    spi_xfer(t, &tx, Some(data))
}

fn known_part(id: u32) -> Option<SpinorInfo> {
    let (name, capacity, erase) = match id {
        0xef3013 => ("W25X40", 512 * 1024, vec![(4096, 0x20), (65536, 0xd8)]),
        0xefc018 => (
            "W25Q128JVEIQ",
            16 * 1024 * 1024,
            vec![(4096, 0x20), (32768, 0x52), (65536, 0xd8)],
        ),
        0xc84011 => (
            "GD25D10B",
            128 * 1024,
            vec![(4096, 0x20), (32768, 0x52), (65536, 0xd8)],
        ),
        0x1c7018 => (
            "EN25QH128",
            16 * 1024 * 1024,
            vec![(4096, 0x20), (65536, 0xd8)],
        ),
        _ => return None,
    };
    Some(SpinorInfo {
        name: name.into(),
        id,
        capacity,
        page_size: 256,
        address_length: 3,
        erase,
        source: "JEDEC table",
        sfdp_capacity: None,
    })
}

fn detect(t: &mut impl SpiTransport) -> Result<SpinorInfo> {
    let mut id = [0; 3];
    spi_xfer(t, &[0x9f], Some(&mut id))?;
    let id = (id[0] as u32) << 16 | (id[1] as u32) << 8 | id[2] as u32;
    if id == 0 || id == 0xffffff {
        return Err(Error::NoSpinor);
    }
    let sfdp = read_sfdp_info(t, id)?;
    if let Some(mut known) = known_part(id) {
        if let Some(sfdp) = sfdp {
            known.sfdp_capacity = Some(sfdp.capacity);
            if sfdp.capacity != known.capacity {
                eprintln!(
                    "JEDEC {id:06x}: ignoring inconsistent SFDP capacity {} (known {} bytes)",
                    sfdp.capacity, known.capacity
                );
            }
        }
        return Ok(known);
    }
    sfdp.ok_or_else(|| {
        error(format!(
            "unknown JEDEC ID {id:06x} and no supported SFDP geometry"
        ))
    })
}

fn read_sfdp_info(t: &mut impl SpiTransport, id: u32) -> Result<Option<SpinorInfo>> {
    let mut header = [0; 8];
    sfdp_read(t, 0, &mut header)?;
    if &header[..4] != b"SFDP" || header[5] != 1 {
        return Ok(None);
    }
    // NPH is byte 6, zero-based; byte 7 is the access protocol (usually FF).
    let mut headers = vec![0; (header[6] as usize + 1) * 8];
    sfdp_read(t, 8, &mut headers)?;
    let mut candidates: Vec<_> = headers
        .as_chunks::<8>()
        .0
        .iter()
        .filter(|h| h[0] == 0 && h[7] == 0xff && h[2] == 1 && h[3] >= 9)
        .collect();
    // Select the newest supported basic parameter revision, never the largest capacity.
    candidates.sort_by_key(|h| std::cmp::Reverse((h[2], h[1], h[3])));
    for h in candidates {
        let addr = h[4] as u32 | (h[5] as u32) << 8 | (h[6] as u32) << 16;
        let mut table = vec![0; h[3] as usize * 4];
        if addr < 8 + headers.len() as u32 {
            continue;
        }
        sfdp_read(t, addr, &mut table)?;
        if let Ok(info) = parse_sfdp(&table, h[1], id) {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

fn parse_sfdp(table: &[u8], minor: u8, id: u32) -> Result<SpinorInfo> {
    let word = |n: usize| -> Result<u32> {
        let bytes = table
            .get((n - 1) * 4..n * 4)
            .ok_or_else(|| error("truncated SFDP table"))?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    };
    let density = word(2)?;
    let capacity = if density & 0x80000000 != 0 {
        let exponent = (density & 0x7fffffff)
            .checked_sub(3)
            .ok_or_else(|| error("invalid SFDP density"))?;
        1u64.checked_shl(exponent)
            .ok_or_else(|| error("SFDP density overflows"))?
    } else {
        (density as u64 + 1) / 8
    };
    if !(128 * 1024..=1u64 << 32).contains(&capacity) || !capacity.is_power_of_two() {
        return Err(error("invalid SFDP capacity"));
    }
    let address_mode = (word(1)? >> 17) & 3;
    // Larger unknown chips need manufacturer-specific 4-byte setup; reject them.
    if capacity > 16 * 1024 * 1024 || address_mode >= 2 {
        return Err(error("SFDP requires unsupported 4-byte address setup"));
    }
    let mut erase = Vec::new();
    for n in [8, 9] {
        let value = word(n)?;
        for shift in [0, 16] {
            let exponent = (value >> shift) & 0xff;
            let opcode = (value >> (shift + 8)) as u8;
            if (12..=18).contains(&exponent) && opcode != 0 && opcode != 0xff {
                erase.push((1u32 << exponent, opcode));
            }
        }
    }
    if erase.is_empty() {
        return Err(error("SFDP has no supported erase type"));
    }
    let page_size = if minor >= 5 {
        1usize << ((word(11)? >> 4) & 0xf)
    } else {
        256
    };
    if !(256..=4096).contains(&page_size) {
        return Err(error("invalid SFDP page size"));
    }
    Ok(SpinorInfo {
        name: "SFDP".into(),
        id,
        capacity,
        page_size,
        address_length: 3,
        erase,
        source: "SFDP",
        sfdp_capacity: Some(capacity),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_en25qh128_and_range_validation() {
        let i = known_part(0x1c7018).unwrap();
        assert_eq!(i.capacity, 0x1000000);
        assert!(i.check_range(0, i.capacity).is_ok());
        assert!(i.check_range(i.capacity, 1).is_err());
        assert!(i.check_range(u64::MAX, 2).is_err());
        assert!(i.check_erase(1, 4096).is_err());
        assert!(i.check_erase(0, 4095).is_err());
        assert!(i.check_erase(0, 4096).is_ok());
        assert!(i.check_erase(0, 0).is_ok());
    }
    #[test]
    fn malformed_sfdp_is_rejected() {
        assert!(parse_sfdp(&[0; 8], 0, 0).is_err());
        let mut table = [0; 44];
        table[4..8].copy_from_slice(&0xffffffffu32.to_le_bytes());
        assert!(parse_sfdp(&table, 5, 0).is_err());
        table[4..8].copy_from_slice(&0x07ffffffu32.to_le_bytes());
        table[28..32].copy_from_slice(&0x0000200cu32.to_le_bytes());
        table[40..44].copy_from_slice(&0x80u32.to_le_bytes());
        assert_eq!(parse_sfdp(&table, 5, 0x1c7018).unwrap().capacity, 0x1000000);
    }
    struct Capture {
        tx: Vec<u8>,
    }
    impl SpiTransport for Capture {
        fn write(&mut self, _: u32, data: &[u8]) -> Result<()> {
            self.tx = data.to_vec();
            Ok(())
        }
        fn run(&mut self, _: &[u8]) -> Result<()> {
            Ok(())
        }
        fn read(&mut self, _: u32, data: &mut [u8]) -> Result<()> {
            if self.tx == [0x9f] {
                data.copy_from_slice(&[0x20, 0x40, 0x18]);
            } else {
                let fixture = include_bytes!("../tests/fixtures/f800-204018-sfdp.bin");
                let addr =
                    (self.tx[1] as usize) << 16 | (self.tx[2] as usize) << 8 | self.tx[3] as usize;
                let bytes = fixture.get(addr..addr + data.len()).ok_or_else(|| {
                    error("probe exceeded captured SFDP; wrong NPH or table pointer")
                })?;
                data.copy_from_slice(bytes);
            }
            Ok(())
        }
    }
    #[test]
    fn captured_f800_sfdp_selects_basic_table_and_16_mib() {
        let info = detect(&mut Capture { tx: Vec::new() }).unwrap();
        assert_eq!(info.capacity, 0x1000000);
        assert_eq!(info.id, 0x204018);
        assert_eq!(info.page_size, 256);
        assert_eq!(info.source, "SFDP");
    }
    struct Failing {
        fail: u8,
    }
    impl SpiTransport for Failing {
        fn write(&mut self, _: u32, _: &[u8]) -> Result<()> {
            if self.fail == 0 {
                Err(error("write"))
            } else {
                Ok(())
            }
        }
        fn run(&mut self, _: &[u8]) -> Result<()> {
            if self.fail == 1 {
                Err(error("run"))
            } else {
                Ok(())
            }
        }
        fn read(&mut self, _: u32, _: &mut [u8]) -> Result<()> {
            Err(error("read"))
        }
    }
    #[test]
    fn transfer_failures_are_propagated() {
        for fail in 0..3 {
            assert!(spi_xfer(&mut Failing { fail }, &[0x9f], Some(&mut [0; 3])).is_err());
        }
        assert!(verify_bytes(0, &[1, 2], &[1, 3]).is_err());
    }
}
