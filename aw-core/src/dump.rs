//! Raw SPI NOR extraction/repacking with an original-image template.
use crate::{
    host,
    layout::{Layout, F800},
    partition::{self, Kind},
};
use anyhow::{bail, Context, Result};
use std::{collections::BTreeMap, fs, path::Path};

pub const LAYOUT_FILE: &str = "layout.json";
pub const TEMPLATE_FILE: &str = ".aw-original.bin";

#[derive(Debug, Clone)]
pub struct PackedPartitions {
    pub layout: Layout,
    pub partitions: BTreeMap<String, Vec<u8>>,
    pub full_image: Vec<u8>,
}

pub fn extract_dump(dump_path: &Path, out_dir: &Path, verbose: bool) -> Result<Layout> {
    let data = fs::read(dump_path).with_context(|| format!("read {}", dump_path.display()))?;
    let layout = Layout::from_dump_profile(&data, &F800)?;
    let output = host::OutputDir::new(out_dir)?;
    let dir = output.path();
    fs::write(dir.join(LAYOUT_FILE), serde_json::to_vec_pretty(&layout)?)?;
    fs::write(dir.join(TEMPLATE_FILE), &data)?;
    for part in &layout.parts {
        if part.size == 0 {
            continue;
        }
        host::safe_name(&part.name)?;
        if format!("{}.bin", part.name) == TEMPLATE_FILE {
            bail!("partition name conflicts with template");
        }
        let start = usize::try_from(part.offset)?;
        let end = usize::try_from(part.offset + part.size)?;
        let file = dir.join(format!("{}.bin", part.name));
        let bytes = data.get(start..end).context("partition exceeds dump")?;
        host::write_new(&file, bytes)?;
        partition::extract(Kind::from_name(&part.name), bytes, &file, verbose)?;
    }
    output.commit()?;
    Ok(layout)
}

pub fn pack_dump(in_dir: &Path, verbose: bool) -> Result<PackedPartitions> {
    let layout: Layout = serde_json::from_slice(&fs::read(in_dir.join(LAYOUT_FILE))?)?;
    layout.validate()?;
    let template = in_dir.join(TEMPLATE_FILE);
    let has_template = template.is_file();
    let mut full_image = if has_template {
        let bytes = fs::read(&template)?;
        if bytes.len() as u64 != layout.flash_size {
            bail!("template size differs from layout");
        }
        bytes
    } else {
        // Old extract trees are accepted only when their partitions cover every byte.
        let mut ranges: Vec<_> = layout.parts.iter().filter(|p| p.size > 0).collect();
        ranges.sort_by_key(|p| p.offset);
        let mut end = 0;
        for p in ranges {
            if p.offset != end {
                bail!("layout has gaps; re-extract to preserve the original image template");
            }
            end += p.size;
        }
        if end != layout.flash_size {
            bail!("layout has an uncovered tail; original image template required");
        }
        vec![0xff; usize::try_from(layout.flash_size)?]
    };
    let mut partitions = BTreeMap::new();
    for part in &layout.parts {
        if part.size == 0 {
            continue;
        }
        let file = host::contained_path(in_dir, &format!("{}.bin", part.name))?;
        let raw = fs::read(&file).with_context(|| format!("read {}", file.display()))?;
        if raw.len() as u64 != part.size {
            bail!("partition '{}' raw size differs from layout", part.name);
        }
        let start = usize::try_from(part.offset)?;
        let raw_modified = has_template && raw.as_slice() != &full_image[start..start + raw.len()];
        let raw_digest = crate::util::md5_hex(&raw);
        let bytes = partition::rebuild(Kind::from_name(&part.name), &file, raw, verbose)?;
        if raw_modified && crate::util::md5_hex(&bytes) != raw_digest {
            bail!(
                "raw partition '{}' and nested editable files conflict; edit one representation",
                part.name
            );
        }
        let bytes = partition::pad(bytes, usize::try_from(part.size)?)?;
        full_image[start..start + bytes.len()].copy_from_slice(&bytes);
        partitions.insert(part.name.clone(), bytes);
    }
    // Changing env.mtdparts requires a matching layout, never an implicit relocation.
    if let Some(bytes) = partitions.get("env") {
        let env = crate::env::Env::parse(bytes)?;
        let expected = Layout::from_mtdparts(
            env.get("mtdparts").context("env has no mtdparts")?,
            layout.flash_size,
            layout.get("env")?.offset,
        )?;
        if expected.parts != layout.parts {
            bail!("env mtdparts and layout.json disagree");
        }
    }
    Ok(PackedPartitions {
        layout,
        partitions,
        full_image,
    })
}
