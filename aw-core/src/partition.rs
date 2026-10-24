//! Shared partition workflows used by raw dumps and IMAGEWTY items.
use crate::{
    bootimg::BootImg,
    bootlogo, egon,
    env::Env,
    host,
    package::{self, Package},
    squash,
};
use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Boot0,
    Package,
    Env,
    Boot,
    Rootfs,
    Bootlogo,
    Opaque,
}
impl Kind {
    pub fn from_name(name: &str) -> Self {
        match name {
            "boot0" => Self::Boot0,
            "package" => Self::Package,
            "env" => Self::Env,
            "boot" => Self::Boot,
            "rootfs" => Self::Rootfs,
            "bootlogo" => Self::Bootlogo,
            _ => Self::Opaque,
        }
    }
}
fn subdir(file: &Path) -> Result<PathBuf> {
    let name = format!(
        "{}.out",
        file.file_name()
            .context("partition filename missing")?
            .to_string_lossy()
    );
    host::contained_path(file.parent().context("partition parent missing")?, &name)
}
pub fn pad(mut bytes: Vec<u8>, size: usize) -> Result<Vec<u8>> {
    if bytes.len() > size {
        bail!(
            "rebuilt payload ({} bytes) exceeds partition/item capacity ({size})",
            bytes.len()
        );
    }
    bytes.resize(size, 0xff);
    Ok(bytes)
}

pub fn extract(kind: Kind, data: &[u8], file: &Path, verbose: bool) -> Result<()> {
    let parent = file.parent().context("partition parent missing")?;
    match kind {
        Kind::Boot0 => {
            egon::verify(data)?;
        }
        Kind::Package => {
            let pkg = Package::parse(data)?;
            let dir = subdir(file)?;
            fs::create_dir(&dir)?;
            for item in &pkg.items {
                host::safe_name(&item.name)?;
                host::write_new(&dir.join(format!("{}.bin", item.name)), pkg.item_data(item))?;
            }
        }
        Kind::Env => host::write_new(
            &parent.join("env.txt"),
            Env::parse(data)?.to_text().as_bytes(),
        )?,
        Kind::Boot => {
            let boot = BootImg::parse(data)?;
            let dir = subdir(file)?;
            fs::create_dir(&dir)?;
            for (name, bytes) in [
                ("kernel.img", boot.kernel),
                ("ramdisk.img", boot.ramdisk),
                ("second.img", boot.second),
            ] {
                // Empty files allow all three components to be edited consistently.
                host::write_new(&dir.join(name), &bytes)?;
            }
        }
        Kind::Rootfs => {
            squash::extract(data, &subdir(file)?)?;
        }
        Kind::Bootlogo => {
            host::write_new(&parent.join("bootlogo.jpg"), bootlogo::extract_jpeg(data)?)?
        }
        Kind::Opaque => {}
    }
    if verbose {
        println!("    validated/unpacked {}", file.display());
    }
    Ok(())
}

pub fn rebuild(kind: Kind, file: &Path, mut raw: Vec<u8>, verbose: bool) -> Result<Vec<u8>> {
    let original = raw.clone();
    let size = raw.len();
    let parent = file.parent().context("partition parent missing")?;
    match kind {
        Kind::Boot0 => {
            egon::verify(&raw)?;
        }
        Kind::Package => {
            let pkg = Package::parse(&raw)?;
            let items: Vec<_> = pkg
                .items
                .iter()
                .map(|i| (i.name.clone(), pkg.item_data(i).to_vec()))
                .collect();
            let dir = subdir(file)?;
            for (name, bytes) in items {
                host::safe_name(&name)?;
                let p = host::contained_path(&dir, &format!("{name}.bin"))?;
                if p.is_file() {
                    let new = fs::read(p)?;
                    if new != bytes {
                        package::replace_item(&mut raw, &name, &new)?;
                    }
                }
            }
        }
        Kind::Env => {
            let mut env = Env::parse(&raw)?;
            let p = host::contained_path(parent, "env.txt")?;
            if p.is_file() {
                let text = fs::read_to_string(p)?;
                if text != env.to_text() {
                    env.vars = Env::from_text(&text).vars;
                    raw = env.build(size)?;
                }
            }
        }
        Kind::Boot => {
            let mut boot = BootImg::parse(&raw)?;
            let dir = subdir(file)?;
            let mut changed = false;
            for (name, bytes) in [
                ("kernel.img", &mut boot.kernel),
                ("ramdisk.img", &mut boot.ramdisk),
                ("second.img", &mut boot.second),
            ] {
                let p = host::contained_path(&dir, name)?;
                if p.is_file() {
                    let new = fs::read(p)?;
                    changed |= new != *bytes;
                    *bytes = new;
                }
            }
            if changed {
                raw = pad(boot.build()?, size)?;
            }
        }
        Kind::Rootfs => {
            let dir = subdir(file)?;
            squash::used_len(&raw)?;
            if dir.is_dir() && squash::tree_modified(&dir)? {
                raw = pad(squash::build(&dir)?, size)?;
            }
        }
        Kind::Bootlogo => {
            let old = bootlogo::extract_jpeg(&raw)?;
            let p = host::contained_path(parent, "bootlogo.jpg")?;
            if p.is_file() {
                let new = fs::read(p)?;
                if new != old {
                    bootlogo::validate(
                        &new,
                        crate::layout::F800.logo_width,
                        crate::layout::F800.logo_height,
                        size,
                    )?;
                    raw = bootlogo::build_partition(&new, size)?;
                }
            }
        }
        Kind::Opaque => {}
    }
    if verbose && raw != original {
        println!("  rebuilt {} ({} bytes)", file.display(), raw.len());
    }
    Ok(raw)
}
