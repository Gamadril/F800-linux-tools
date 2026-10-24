use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use xfel_spi_nor::{Error, SpinorFlash};

fn parse_u64(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        s.parse::<u64>().map_err(|e| e.to_string())
    }
}

#[derive(Parser)]
#[command(
    name = "xfel_spi_nor",
    version,
    about = "Allwinner V851S/V853 (sun8iw21) SPI NOR over USB FEL"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// FEL chip id + SPI NOR detect
    Version,
    /// Detect SPI NOR (name + capacity)
    Spinor,
    /// Read SPI NOR to file or stdout (-)
    Read {
        #[arg(value_parser = parse_u64)]
        address: u64,
        #[arg(value_parser = parse_u64)]
        length: u64,
        file: PathBuf,
    },
    /// Write file to SPI NOR (erases covered sectors first, like xfel)
    Write {
        #[arg(value_parser = parse_u64)]
        address: u64,
        file: PathBuf,
        /// Explicitly authorize erase/program operations
        #[arg(long)]
        yes: bool,
        /// Validate and display the exact erase range without writing
        #[arg(long)]
        dry_run: bool,
    },
    /// Erase SPI NOR range
    Erase {
        #[arg(value_parser = parse_u64)]
        address: u64,
        #[arg(value_parser = parse_u64)]
        length: u64,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Save the raw 256-byte SFDP area for diagnosis
    Sfdp { file: PathBuf },
    /// Reset the SoC via watchdog soft reset
    Reset,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    if matches!(
        &cli.cmd,
        Cmd::Write {
            yes: false,
            dry_run: false,
            ..
        } | Cmd::Erase {
            yes: false,
            dry_run: false,
            ..
        }
    ) {
        return Err(Error::Msg(
            "erase/program requires --yes; inspect the operation first with --dry-run".into(),
        ));
    }
    let mut dev = SpinorFlash::open()?;
    match cli.cmd {
        Cmd::Version => {
            println!("AWUSBFEX ID=0x{:08x} (V851S/V853)", dev.version_id());
            let info = dev.detect()?;
            println!("spinor {} ({} bytes)", info.name, info.capacity);
            println!(
                "JEDEC ID={:06x}, geometry={}{}",
                info.jedec_id,
                info.source,
                info.sfdp_capacity
                    .map(|n| format!(", SFDP={n} bytes"))
                    .unwrap_or_default()
            );
        }
        Cmd::Spinor => {
            let info = dev.detect()?;
            println!("{} {}", info.name, info.capacity);
        }
        Cmd::Read {
            address,
            length,
            file,
        } => {
            if file.as_os_str() != "-" && file.exists() {
                return Err(Error::Msg(format!(
                    "output already exists: {}",
                    file.display()
                )));
            }
            dev.validate_read_range(address, length)?;
            let len =
                usize::try_from(length).map_err(|_| Error::Msg("read length too large".into()))?;
            let mut buf = Vec::new();
            buf.try_reserve_exact(len)
                .map_err(|e| Error::Msg(e.to_string()))?;
            buf.resize(len, 0);
            dev.read(address, &mut buf)?;
            if file.as_os_str() == "-" {
                std::io::stdout()
                    .write_all(&buf)
                    .map_err(|e| Error::Msg(e.to_string()))?;
            } else {
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&file)
                    .map_err(|e| Error::Msg(e.to_string()))?;
                output
                    .write_all(&buf)
                    .map_err(|e| Error::Msg(e.to_string()))?;
                output.sync_all().map_err(|e| Error::Msg(e.to_string()))?;
            }
        }
        Cmd::Write {
            address,
            file,
            dry_run,
            ..
        } => {
            let mut f = File::open(&file).map_err(|e| Error::Msg(e.to_string()))?;
            let mut data = Vec::new();
            f.read_to_end(&mut data)
                .map_err(|e| Error::Msg(e.to_string()))?;
            dev.validate_erase_range(address, data.len() as u64)?;
            eprintln!(
                "Erase/program {address:#x}..{:#x} ({} bytes), then read back and verify",
                address + data.len() as u64,
                data.len()
            );
            if !dry_run {
                dev.write(address, &data)?;
            }
        }
        Cmd::Erase {
            address,
            length,
            dry_run,
            ..
        } => {
            dev.validate_erase_range(address, length)?;
            eprintln!(
                "Erase {address:#x}..{:#x} ({length} bytes)",
                address + length
            );
            if !dry_run {
                dev.erase(address, length)?;
            }
        }
        Cmd::Sfdp { file } => {
            let mut data = [0; 256];
            dev.read_sfdp(0, &mut data)?;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|e| Error::Msg(e.to_string()))?;
            output
                .write_all(&data)
                .map_err(|e| Error::Msg(e.to_string()))?;
        }
        Cmd::Reset => {
            dev.reset()?;
        }
    }
    Ok(())
}
