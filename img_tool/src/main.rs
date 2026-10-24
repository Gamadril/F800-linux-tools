#![cfg_attr(target_arch = "wasm32", no_main)]

#[cfg(target_arch = "wasm32")]
pub use img_tool::*;

#[cfg(not(target_arch = "wasm32"))]
mod cli_app {
    use anyhow::{bail, Context, Result};
    use aw_core::bootlogo::{self, DEFAULT_HEIGHT, DEFAULT_WIDTH};
    use aw_core::dump::pack_dump;
    use aw_core::host;
    use aw_core::imagewty::{names, Image, Item};
    use aw_core::package::Package;
    use clap::{Parser, Subcommand};
    use img_tool::{build_bootlogo_image, extract_bootlogo_jpeg, DEFAULT_BOOTLOGO_PART_SIZE};
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::path::{Path, PathBuf};

    const DEFAULT_IMG_NAME: &str = "firmware_update.img";
    const DEFAULT_LOGO_IMG_NAME: &str = "bootlogo_update.img";
    const MANIFEST_FILE: &str = "image_items.json";

    #[derive(Parser, Debug)]
    #[command(
        name = "img_tool",
        version,
        about = "Extract, pack, and build Allwinner IMAGEWTY (.img) SD-card update images (including bootlogo-only updates)"
    )]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[derive(Subcommand, Debug)]
    enum Cmd {
        /// Extract an Allwinner IMAGEWTY (.img) firmware update image into items and nested contents
        Extract {
            /// Input .img file
            input: PathBuf,
            /// Output directory
            output: PathBuf,
            #[arg(short, long)]
            verbose: bool,
        },
        /// Pack an extracted IMAGEWTY directory back into an update .img (with md5sum trailer)
        Pack {
            /// Input directory created by `img_tool extract`
            input: PathBuf,
            /// Output .img file (default: firmware_update.img)
            output: Option<PathBuf>,
            #[arg(short, long)]
            verbose: bool,
        },
        /// Build an update .img from a `dump_tool extract` directory
        FromDump {
            /// Input directory created by `dump_tool extract`
            input: PathBuf,
            /// Output .img file (default: firmware_update.img)
            output: Option<PathBuf>,
            /// Comma-separated list of partitions to include (default: boot,rootfs,bootlogo,env,userdata,boot0,package; or e.g. --only bootlogo)
            #[arg(long)]
            only: Option<String>,
            #[arg(short, long)]
            verbose: bool,
        },
        /// Build a bootlogo-only SD update .img from a JPEG file (or extract the JPEG from a .img / partition)
        Bootlogo {
            /// Input baseline JPEG (1024x600) to wrap into an update .img, or existing .img/bootlogo.bin when --extract is used
            input: PathBuf,
            /// Output .img file (default: bootlogo_update.img) or output .jpg when --extract is set
            output: Option<PathBuf>,
            /// Extract JPEG from an existing update .img or bootlogo.bin instead of building an .img
            #[arg(long)]
            extract: bool,
            /// Expected image width (default: 1024)
            #[arg(long, default_value_t = DEFAULT_WIDTH)]
            width: u32,
            /// Expected image height (default: 600)
            #[arg(long, default_value_t = DEFAULT_HEIGHT)]
            height: u32,
            /// Bootlogo partition size in bytes (default: 131072 = 128K)
            #[arg(long, default_value_t = DEFAULT_BOOTLOGO_PART_SIZE)]
            part_size: usize,
            /// Skip strict dimension check
            #[arg(long)]
            no_check_dims: bool,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct ItemMeta {
        pub file: String,
        pub main_type: String,
        pub sub_type: String,
        pub name: String,
        #[serde(default)]
        pub record: Vec<u8>,
        #[serde(default)]
        pub original_md5: Option<String>,
    }

    fn filename_for_subtype(sub_type: &str, name: &str, idx: usize) -> String {
        if !name.is_empty() {
            return name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
        }
        match sub_type {
            names::BOOT => "boot.fex".into(),
            names::ROOTFS => "rootfs.fex".into(),
            names::BOOTLOGO => "bootlogo.fex".into(),
            names::ENV => "env.fex".into(),
            names::USERDATA => "userdata.fex".into(),
            names::OVERLAY => "overlay.fex".into(),
            names::BOOT0 => "boot0_nor.fex".into(),
            names::BOOTPKG => "boot_package_nor.fex".into(),
            _ => format!("item_{idx}_{sub_type}.bin"),
        }
    }

    fn kind_for_subtype(subtype: &str) -> aw_core::partition::Kind {
        use aw_core::partition::Kind;
        match subtype {
            names::BOOT0 => Kind::Boot0,
            names::BOOTPKG => Kind::Package,
            names::ENV => Kind::Env,
            names::BOOT => Kind::Boot,
            names::ROOTFS => Kind::Rootfs,
            names::BOOTLOGO => Kind::Bootlogo,
            _ => Kind::Opaque,
        }
    }

    fn extract_nested(file_path: &Path, sub_type: &str, data: &[u8], verbose: bool) -> Result<()> {
        aw_core::partition::extract(kind_for_subtype(sub_type), data, file_path, verbose)
    }

    fn repack_nested(dir: &Path, meta: &ItemMeta, raw: Vec<u8>, verbose: bool) -> Result<Vec<u8>> {
        aw_core::partition::rebuild(
            kind_for_subtype(&meta.sub_type),
            &dir.join(&meta.file),
            raw,
            verbose,
        )
    }

    fn cmd_extract(input: &Path, output: &Path, verbose: bool) -> Result<()> {
        let bytes =
            fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;
        let (img, md5_ok) = Image::parse(&bytes)?;
        if let Some(ok) = md5_ok {
            if !ok {
                bail!("IMAGEWTY md5sum trailer mismatch!");
            }
            if verbose {
                println!("  verified md5sum trailer: OK");
            }
        }
        let staging = host::OutputDir::new(output)?;
        let dir = staging.path();
        host::write_new(&dir.join("image_header.bin"), &img.header)?;

        let mut metas = Vec::new();
        for (idx, it) in img.items.iter().enumerate() {
            let fname = filename_for_subtype(&it.sub_type, &it.name, idx);
            host::safe_name(&fname)?;
            if matches!(fname.as_str(), "image_header.bin" | MANIFEST_FILE) {
                bail!("reserved item filename {fname}");
            }
            let fpath = dir.join(&fname);
            host::write_new(&fpath, &it.data)?;
            if verbose {
                println!(
                    "  item[{idx}] sub_type={:<16} main_type={:<8} len={:#x} -> {}",
                    it.sub_type,
                    it.main_type,
                    it.data.len(),
                    fname
                );
            }
            extract_nested(&fpath, &it.sub_type, &it.data, verbose)?;
            metas.push(ItemMeta {
                file: fname,
                main_type: it.main_type.clone(),
                sub_type: it.sub_type.clone(),
                name: it.name.clone(),
                record: it.record.clone(),
                original_md5: Some(aw_core::util::md5_hex(&it.data)),
            });
        }
        host::write_new(
            &dir.join(MANIFEST_FILE),
            serde_json::to_string_pretty(&metas)?.as_bytes(),
        )?;
        staging.commit()?;
        println!(
            "Extracted {} IMAGEWTY item(s) to {}",
            metas.len(),
            output.display()
        );
        Ok(())
    }

    fn cmd_pack(input: &Path, output: &Path, verbose: bool) -> Result<()> {
        let manifest_path = input.join(MANIFEST_FILE);
        let metas: Vec<ItemMeta> = serde_json::from_str(
            &fs::read_to_string(&manifest_path)
                .with_context(|| format!("missing {}", manifest_path.display()))?,
        )?;
        let mut items = Vec::new();
        for m in &metas {
            let fpath = host::contained_path(input, &m.file)?;
            let raw = fs::read(&fpath).with_context(|| format!("missing {}", fpath.display()))?;
            let raw_digest = aw_core::util::md5_hex(&raw);
            let data = repack_nested(input, m, raw, verbose)?;
            if m.original_md5.as_ref().is_some_and(|d| *d != raw_digest)
                && aw_core::util::md5_hex(&data) != raw_digest
            {
                bail!(
                    "raw item '{}' and nested editable files conflict; edit one representation",
                    m.file
                );
            }
            let mut item = Item::new(&m.sub_type, &m.main_type, &m.name, data);
            item.record = m.record.clone();
            items.push(item);
        }
        let header_path = host::contained_path(input, "image_header.bin")?;
        let img = Image {
            items,
            header: if header_path.is_file() {
                fs::read(header_path)?
            } else {
                Vec::new()
            },
        };
        let out_bytes = img.build(true)?;
        host::write_new(output, &out_bytes)?;
        println!(
            "Packed {} item(s) into {} ({} bytes, with md5sum trailer)",
            metas.len(),
            output.display(),
            out_bytes.len()
        );
        Ok(())
    }

    fn map_dump_part_to_item(
        part_name: &str,
    ) -> Option<(&'static str, &'static str, &'static str)> {
        match part_name {
            "boot" => Some((names::BOOT, "RFSFAT16", "boot.fex")),
            "rootfs" => Some((names::ROOTFS, "RFSFAT16", "rootfs.fex")),
            "bootlogo" => Some((names::BOOTLOGO, "RFSFAT16", "bootlogo.fex")),
            "env" => Some((names::ENV, "RFSFAT16", "env.fex")),
            "userdata" => Some((names::USERDATA, "RFSFAT16", "userdata.fex")),
            "boot0" => Some((names::BOOT0, "12345678", "boot0_nor.fex")),
            "package" => Some((names::BOOTPKG, "12345678", "boot_package_nor.fex")),
            _ => None,
        }
    }

    fn cmd_from_dump(input: &Path, output: &Path, only: Option<&str>, verbose: bool) -> Result<()> {
        let packed = pack_dump(input, verbose)?;
        let filter: Option<Vec<String>> = only.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        });

        // Order matches cdr's upgradeThread table: boot, rootfs, bootlogo, env, userdata, boot0, package
        let order = [
            "boot", "rootfs", "bootlogo", "env", "userdata", "boot0", "package",
        ];
        if let Some(filter) = &filter {
            for name in filter {
                if !order.iter().any(|p| p.eq_ignore_ascii_case(name)) {
                    bail!("unknown partition selection: {name}");
                }
                if !packed
                    .partitions
                    .keys()
                    .any(|p| p.eq_ignore_ascii_case(name))
                {
                    bail!("selected partition missing: {name}");
                }
            }
        }
        let mut items = Vec::new();
        for pname in order {
            if let Some(ref f) = filter {
                if !f.iter().any(|x| x.eq_ignore_ascii_case(pname)) {
                    continue;
                }
            }
            let Some(data) = packed.partitions.get(pname) else {
                continue;
            };
            let (sub_type, main_type, fname) = map_dump_part_to_item(pname).unwrap();
            let mut payload = data.clone();
            if pname == "boot0" {
                // boot0 in NOR is 64K slot; eGON header has actual length (0x9000)
                if let Ok(len) = aw_core::egon::verify(&payload) {
                    payload.truncate(len as usize);
                }
            } else if pname == "package" {
                if let Ok(pkg) = Package::parse(&payload) {
                    payload.truncate(pkg.valid_len);
                }
            }
            if verbose {
                println!(
                    "  adding partition {:<10} as sub_type={:<16} ({} bytes)",
                    pname,
                    sub_type,
                    payload.len()
                );
            }
            items.push(Item::new(sub_type, main_type, fname, payload));
        }

        if items.is_empty() {
            bail!("no matching partitions selected for IMAGEWTY output");
        }

        // Omit the MBR item according to the static cdr updater model.
        let img = Image::new(items);
        let out_bytes = img.build(true)?;
        host::write_new(output, &out_bytes)?;
        println!(
            "Built SD update image {} ({} bytes, {} item(s), with md5sum trailer)",
            output.display(),
            out_bytes.len(),
            img.items.len()
        );
        Ok(())
    }

    fn cmd_bootlogo(
        input: &Path,
        output: Option<PathBuf>,
        extract_mode: bool,
        width: u32,
        height: u32,
        part_size: usize,
        no_check_dims: bool,
    ) -> Result<()> {
        let in_bytes =
            fs::read(input).with_context(|| format!("failed to read {}", input.display()))?;

        if extract_mode {
            let out_path = output.unwrap_or_else(|| PathBuf::from("bootlogo.jpg"));
            let jpg = extract_bootlogo_jpeg(&in_bytes)?;
            let info = bootlogo::jpeg_info(&jpg)?;
            host::write_new(&out_path, &jpg)?;
            println!(
                "Extracted bootlogo JPEG ({}x{}, progressive={}, {} bytes) -> {}",
                info.width,
                info.height,
                info.progressive,
                jpg.len(),
                out_path.display()
            );
            return Ok(());
        }

        let out_path = output.unwrap_or_else(|| PathBuf::from(DEFAULT_LOGO_IMG_NAME));
        let info =
            bootlogo::jpeg_info(&in_bytes).context("input file is not a valid JPEG image")?;
        let out_bytes = build_bootlogo_image(&in_bytes, width, height, part_size, !no_check_dims)?;
        host::write_new(&out_path, &out_bytes)?;
        println!(
            "Built bootlogo-only SD update image: {} ({} bytes, JPEG {}x{} = {} bytes, with md5sum trailer)",
            out_path.display(),
            out_bytes.len(),
            info.width,
            info.height,
            info.end
        );
        println!(
            "Copy {} to the root of a FAT32 SD card and insert it into the device.",
            out_path.display()
        );
        Ok(())
    }

    pub fn run() -> Result<()> {
        let cli = Cli::parse();
        match cli.cmd {
            Cmd::Extract {
                input,
                output,
                verbose,
            } => cmd_extract(&input, &output, verbose),
            Cmd::Pack {
                input,
                output,
                verbose,
            } => {
                let out = output.unwrap_or_else(|| PathBuf::from(DEFAULT_IMG_NAME));
                cmd_pack(&input, &out, verbose)
            }
            Cmd::FromDump {
                input,
                output,
                only,
                verbose,
            } => {
                let out = output.unwrap_or_else(|| PathBuf::from(DEFAULT_IMG_NAME));
                cmd_from_dump(&input, &out, only.as_deref(), verbose)
            }
            Cmd::Bootlogo {
                input,
                output,
                extract,
                width,
                height,
                part_size,
                no_check_dims,
            } => cmd_bootlogo(
                &input,
                output,
                extract,
                width,
                height,
                part_size,
                no_check_dims,
            ),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    cli_app::run()
}
