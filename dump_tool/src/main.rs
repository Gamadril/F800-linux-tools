use anyhow::Result;
use aw_core::dump::{extract_dump, pack_dump};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "dump_tool",
    version,
    about = "Extract and repack Allwinner ARM (V851S/V853, Tina Linux) SPI NOR flash dumps"
)]
struct Args {
    /// Processing mode
    #[arg(value_enum)]
    mode: Mode,
    /// Input dump file (extract) or extracted directory (pack)
    input: PathBuf,
    /// Output directory (extract) or output dump file (pack)
    output: PathBuf,
    #[arg(short, long)]
    verbose: bool,
    /// Show what would be done without executing
    #[arg(short, long)]
    dry_run: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum Mode {
    /// Extract a raw SPI NOR flash dump into partitions and nested trees
    Extract,
    /// Repack an extracted dump directory back into a raw SPI NOR flash image
    Pack,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.dry_run {
        match args.mode {
            Mode::Extract => {
                aw_core::host::require_absent(&args.output)?;
                let bytes = std::fs::read(&args.input)?;
                aw_core::layout::Layout::from_dump_profile(&bytes, &aw_core::layout::F800)?;
            }
            Mode::Pack => {
                aw_core::host::require_absent(&args.output)?;
                let _ = pack_dump(&args.input, args.verbose)?;
            }
        }
        match args.mode {
            Mode::Extract => {
                println!("DRY RUN MODE: No files will be modified");
                println!("  Input dump: {}", args.input.display());
                println!("  Output directory: {}", args.output.display());
                println!("  Operations planned:");
                println!("    1. Parse U-Boot env and mtdparts layout at 0x080000");
                println!("    2. Extract boot0.bin, package.bin (+ package.bin.out/), env.bin (+ env.txt)");
                println!("    3. Extract boot.bin (+ boot.bin.out/kernel.img)");
                println!("    4. Extract rootfs.bin (+ rootfs.bin.out/ SquashFS tree)");
                println!("    5. Extract userdata.bin and bootlogo.bin (+ bootlogo.jpg)");
            }
            Mode::Pack => {
                println!("DRY RUN MODE: No files will be modified");
                println!("  Input directory: {}", args.input.display());
                println!("  Output dump: {}", args.output.display());
                println!("  Operations planned:");
                println!("    1. Rebuild modified sub-items (package.bin.out/, env.txt, boot.bin.out/, rootfs.bin.out/, bootlogo.jpg)");
                println!("    2. Splice all partitions at their layout.json offsets into 16 MB NOR image");
            }
        }
        return Ok(());
    }

    match args.mode {
        Mode::Extract => {
            println!("Extracting dump {:?} to {:?}", args.input, args.output);
            let layout = extract_dump(&args.input, &args.output, args.verbose)?;
            println!(
                "Extracted {} partitions ({:#x} bytes total)",
                layout.parts.len(),
                layout.flash_size
            );
        }
        Mode::Pack => {
            println!("Packing dump {:?} to {:?}", args.input, args.output);
            let packed = pack_dump(&args.input, args.verbose)?;
            aw_core::host::write_new(&args.output, &packed.full_image)?;
            println!(
                "Repacked flash dump written to {:?} ({} bytes)",
                args.output,
                packed.full_image.len()
            );
        }
    }

    Ok(())
}
