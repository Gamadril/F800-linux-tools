//! SquashFS (rootfs) extract / rebuild on top of `backhand`.
//!
//! Extract writes the tree plus a `manifest.tsv` (type, mode, uid, gid, mtime, md5, path, extra)
//! which preserves ownership, special files (device nodes cannot be created without root) and
//! allows detecting whether the tree was modified.

use crate::host;
use crate::util::md5_hex;
use anyhow::{bail, Context, Result};
use backhand::compression::Compressor;
use backhand::{FilesystemCompressor, FilesystemReader, FilesystemWriter, InnerNode, NodeHeader};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "manifest.tsv";
const ORIGINAL_MANIFEST: &str = ".aw-manifest.original";
const TEMPLATE: &str = ".aw-squash.bin";
const VIRTUAL_SYMLINKS: &str = ".aw-virtual-symlinks";
fn reserved(path: &str) -> bool {
    matches!(
        path,
        MANIFEST | ORIGINAL_MANIFEST | TEMPLATE | VIRTUAL_SYMLINKS
    )
}

// Windows preserves symlinks in the manifest instead of requiring administrator
// privileges or Developer Mode to materialize Unix links in the editable tree.
fn virtual_kind(kind: char, virtual_symlinks: bool) -> bool {
    matches!(kind, 'c' | 'b' | 'p' | 's') || (virtual_symlinks && kind == 'l')
}

/// Number of bytes actually used by a squashfs image (superblock `bytes_used`).
pub fn used_len(img: &[u8]) -> Result<usize> {
    if img.len() < 96 || &img[..4] != b"hsqs" {
        bail!("not a squashfs image (magic 'hsqs' missing)");
    }
    let len = usize::try_from(u64::from_le_bytes(img[0x28..0x30].try_into().unwrap()))?;
    if len < 96 || len > img.len() {
        bail!("SquashFS bytes_used exceeds image bounds");
    }
    Ok(len)
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: char, // d f l c b p s
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u32,
    pub md5: String,
    pub path: String,
    pub extra: String, // symlink target / device number
}

pub struct Manifest {
    pub compressor: String,
    pub block_size: u32,
    pub mod_time: u32,
    pub entries: Vec<Entry>,
}

impl Manifest {
    fn to_text(&self) -> String {
        let mut s = format!(
            "#aw-squash\t{}\t{}\t{}\n",
            self.compressor, self.block_size, self.mod_time
        );
        for e in &self.entries {
            s += &format!(
                "{}\t{:o}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                e.kind, e.mode, e.uid, e.gid, e.mtime, e.md5, e.extra, e.path
            );
        }
        s
    }

    fn from_text(t: &str) -> Result<Self> {
        let mut lines = t.lines();
        let h: Vec<&str> = lines
            .next()
            .context("empty manifest")?
            .split('\t')
            .collect();
        if h.len() != 4 || h[0] != "#aw-squash" {
            bail!("bad manifest header");
        }
        let mut entries = Vec::new();
        let mut paths = std::collections::BTreeSet::new();
        for l in lines {
            let f: Vec<&str> = l.splitn(8, '\t').collect();
            if f.len() != 8 {
                bail!("bad manifest line: {l}");
            }
            let kind = f[0].chars().next().context("empty manifest kind")?;
            if f[0].len() != 1 || !"dflcbps".contains(kind) {
                bail!("invalid manifest kind");
            }
            if f[7] == "." {
                if kind != 'd' {
                    bail!("manifest root must be a directory");
                }
            } else {
                host::relative_path(f[7])?;
            }
            if reserved(f[7]) || !paths.insert(f[7].to_string()) {
                bail!("reserved or duplicate manifest path");
            }
            entries.push(Entry {
                kind,
                mode: u16::from_str_radix(f[1], 8)?,
                uid: f[2].parse()?,
                gid: f[3].parse()?,
                mtime: f[4].parse()?,
                md5: f[5].into(),
                extra: f[6].into(),
                path: f[7].into(),
            });
        }
        let block_size: u32 = h[2].parse()?;
        if !block_size.is_power_of_two() || !(4096..=1048576).contains(&block_size) {
            bail!("invalid SquashFS block size");
        }
        Ok(Manifest {
            compressor: h[1].into(),
            block_size,
            mod_time: h[3].parse()?,
            entries,
        })
    }
}

fn rel(p: &Path) -> Result<String> {
    host::manifest_path(p.strip_prefix("/").unwrap_or(p))
}

/// Extract into a new directory; root metadata and compression options are retained.
pub fn extract(img: &[u8], out: &Path) -> Result<usize> {
    extract_impl(img, out, cfg!(unix))
}

fn extract_impl(img: &[u8], out: &Path, materialize_symlinks: bool) -> Result<usize> {
    let img = &img[..used_len(img)?];
    let fs_reader = FilesystemReader::from_reader(BufReader::new(Cursor::new(img.to_vec())))
        .context("reading squashfs")?;
    let output = host::OutputDir::new(out)?;
    let out = output.path();
    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for node in fs_reader.files() {
        let path = rel(&node.fullpath)?;
        if path.is_empty() {
            let h = &node.header;
            entries.push(Entry {
                kind: 'd',
                mode: h.permissions,
                uid: h.uid,
                gid: h.gid,
                mtime: h.mtime,
                md5: String::new(),
                path: ".".into(),
                extra: String::new(),
            });
            continue;
        }
        if reserved(&path) || !seen.insert(path.clone()) {
            bail!("reserved or duplicate SquashFS path {path:?}");
        }
        let dst = host::contained_path(out, &path)?;
        let h = &node.header;
        let (kind, extra, md5) = match &node.inner {
            InnerNode::Dir(_) => {
                fs::create_dir_all(&dst)?;
                ('d', String::new(), String::new())
            }
            InnerNode::File(f) => {
                let mut data = Vec::new();
                fs_reader.file(f).reader().read_to_end(&mut data)?;
                host::write_new(&dst, &data)?;
                ('f', String::new(), md5_hex(&data))
            }
            InnerNode::Symlink(s) => {
                let target = s.link.to_string_lossy().into_owned();
                if target.contains(['\t', '\n', '\r']) {
                    bail!("symlink target cannot be represented in manifest");
                }
                #[cfg(unix)]
                if materialize_symlinks {
                    symlink(&target, &dst)?;
                }
                ('l', target, String::new())
            }
            InnerNode::CharacterDevice(d) => ('c', d.device_number.to_string(), String::new()),
            InnerNode::BlockDevice(d) => ('b', d.device_number.to_string(), String::new()),
            InnerNode::NamedPipe => ('p', String::new(), String::new()),
            InnerNode::Socket => ('s', String::new(), String::new()),
        };
        if kind == 'd' || kind == 'f' {
            // keep owner rwx so the tree stays editable; real mode is kept in the manifest
            host::make_editable(&dst, h.permissions)?;
        }
        entries.push(Entry {
            kind,
            mode: h.permissions,
            uid: h.uid,
            gid: h.gid,
            mtime: h.mtime,
            md5,
            path,
            extra,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let n = entries.len();
    let comp = format!(
        "{:?}",
        fs_reader
            .compressor
            .context("SquashFS compressor missing")?
    )
    .to_lowercase();
    let m = Manifest {
        compressor: comp,
        block_size: fs_reader.block_size,
        mod_time: fs_reader.mod_time,
        entries,
    };
    let text = m.to_text();
    fs::write(out.join(MANIFEST), &text)?;
    fs::write(out.join(ORIGINAL_MANIFEST), &text)?;
    fs::write(out.join(TEMPLATE), img)?;
    if !materialize_symlinks {
        fs::write(
            out.join(VIRTUAL_SYMLINKS),
            b"Symlink targets are stored in manifest.tsv\n",
        )?;
    }
    output.commit()?;
    Ok(n)
}

/// True if the extracted tree differs from what the manifest recorded.
pub fn tree_modified(dir: &Path) -> Result<bool> {
    let virtual_symlinks = cfg!(windows) || dir.join(VIRTUAL_SYMLINKS).is_file();
    let text = fs::read_to_string(dir.join(MANIFEST)).context("manifest.tsv missing")?;
    let m = Manifest::from_text(&text)?;
    if dir.join(ORIGINAL_MANIFEST).is_file()
        && fs::read_to_string(dir.join(ORIGINAL_MANIFEST))? != text
    {
        return Ok(true);
    }
    let mut known: BTreeMap<&str, &Entry> = BTreeMap::new();
    for e in &m.entries {
        known.insert(&e.path, e);
    }
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(&d)? {
            let p = ent?.path();
            let r = host::manifest_path(p.strip_prefix(dir)?)?;
            if reserved(&r) {
                continue;
            }
            let md = fs::symlink_metadata(&p)?;
            if md.is_dir() {
                stack.push(p.clone());
            }
            seen += 1;
            match known.get(r.as_str()) {
                None => return Ok(true),
                Some(e) => {
                    if e.kind == 'd' && !md.is_dir() {
                        return Ok(true);
                    }
                    if e.kind == 'f' && (!md.is_file() || md5_hex(&fs::read(&p)?) != e.md5) {
                        return Ok(true);
                    }
                    if e.kind == 'l'
                        && (!md.file_type().is_symlink()
                            || fs::read_link(&p)?.to_string_lossy() != e.extra)
                    {
                        return Ok(true);
                    }
                }
            }
        }
    }
    let special = m
        .entries
        .iter()
        .filter(|e| virtual_kind(e.kind, virtual_symlinks))
        .count();
    let root = usize::from(m.entries.iter().any(|e| e.path == "."));
    Ok(seen + special + root != m.entries.len())
}

/// Build a squashfs image from an extracted tree + manifest.
pub fn build(dir: &Path) -> Result<Vec<u8>> {
    let virtual_symlinks = cfg!(windows) || dir.join(VIRTUAL_SYMLINKS).is_file();
    let m = Manifest::from_text(
        &fs::read_to_string(dir.join(MANIFEST)).context("manifest.tsv missing")?,
    )?;
    let data = fs::read(dir.join(TEMPLATE)).context(
        "SquashFS template missing; re-extract to preserve metadata and compression options",
    )?;
    let mut source = FilesystemReader::from_reader(BufReader::new(Cursor::new(data)))?;
    source
        .root
        .nodes
        .retain(|node| node.fullpath == Path::new("/"));
    let root_node = source
        .root
        .nodes
        .first_mut()
        .context("SquashFS root missing")?;
    if let Some(root) = m.entries.iter().find(|e| e.path == ".") {
        root_node.header = NodeHeader::new(root.mode, root.uid, root.gid, root.mtime);
        for id in [root.uid, root.gid] {
            if !source.id_table.iter().any(|entry| entry.num == id) {
                source.id_table.push(backhand::v4::id::Id::new(id));
            }
        }
    }
    let mut w = FilesystemWriter::from_fs_reader(&source)?;
    let comp = match m.compressor.as_str() {
        "xz" | "some(xz)" => Compressor::Xz,
        "gzip" | "some(gzip)" => Compressor::Gzip,
        "zstd" | "some(zstd)" => Compressor::Zstd,
        other => bail!("unsupported compressor '{other}'"),
    };
    if source.compressor != Some(comp.into()) {
        bail!("changing SquashFS compressor requires a new verified compression profile");
    }
    let options = source.compression_options;
    w.set_emit_compression_options(options.is_some());
    w.set_compressor(FilesystemCompressor::new(comp, options)?);
    w.set_block_size(m.block_size);
    w.set_time(m.mod_time);

    let mut known: BTreeMap<String, Entry> = BTreeMap::new();
    for e in &m.entries {
        known.insert(e.path.clone(), e.clone());
    }
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in fs::read_dir(&d)? {
            let p = ent?.path();
            let r = host::manifest_path(p.strip_prefix(dir)?)?;
            if reserved(&r) {
                continue;
            }
            if fs::symlink_metadata(&p)?.is_dir() {
                stack.push(p.clone());
            }
            paths.push((r, p));
        }
    }
    let mut specials: Vec<&Entry> = m
        .entries
        .iter()
        .filter(|e| virtual_kind(e.kind, virtual_symlinks))
        .collect();
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    specials.sort_by(|a, b| a.path.cmp(&b.path));

    for (r, p) in &paths {
        let md = fs::symlink_metadata(p)?;
        let (mode, uid, gid, mtime) = match known.get(r) {
            Some(e) => (e.mode, e.uid, e.gid, e.mtime),
            None => (host::new_file_mode(&md), 0, 0, m.mod_time),
        };
        let hdr = NodeHeader::new(mode, uid, gid, mtime);
        let sq = format!("/{r}");
        if md.file_type().is_symlink() {
            w.push_symlink(fs::read_link(p)?, &sq, hdr)?;
        } else if md.is_dir() {
            w.push_dir(&sq, hdr)?;
        } else {
            w.push_file_from_path(p, &sq, hdr)?;
        }
    }
    for e in specials {
        let hdr = NodeHeader::new(e.mode, e.uid, e.gid, e.mtime);
        let sq = format!("/{}", e.path);
        let dev: u32 = if matches!(e.kind, 'c' | 'b') {
            e.extra.parse().context("invalid device number")?
        } else {
            0
        };
        match e.kind {
            'l' => w.push_symlink(&e.extra, &sq, hdr)?,
            'c' => w.push_char_device(dev, &sq, hdr)?,
            'b' => w.push_block_device(dev, &sq, hdr)?,
            'p' => w.push_fifo(&sq, hdr)?,
            _ => w.push_socket(&sq, hdr)?,
        }
    }
    let mut out = Cursor::new(Vec::new());
    w.write(&mut out)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_symlinks_rebuild_without_host_symlink_privileges() {
        let dir = std::env::temp_dir().join(format!("aw-virtual-links-{}", std::process::id()));
        let mut writer = FilesystemWriter::default();
        writer.set_time(123);
        writer
            .push_file(
                Cursor::new(b"old"),
                "/file",
                NodeHeader::new(0o644, 0, 0, 123),
            )
            .unwrap();
        writer
            .push_symlink("file", "/link", NodeHeader::new(0o777, 0, 0, 123))
            .unwrap();
        let mut image = Cursor::new(Vec::new());
        writer.write(&mut image).unwrap();
        extract_impl(&image.into_inner(), &dir, false).unwrap();
        assert!(!dir.join("link").exists());
        assert!(!tree_modified(&dir).unwrap());
        fs::write(dir.join("file"), b"changed").unwrap();
        assert!(tree_modified(&dir).unwrap());
        let image = build(&dir).unwrap();
        let reader = FilesystemReader::from_reader(BufReader::new(Cursor::new(image))).unwrap();
        assert!(reader.files().any(
            |node| matches!(&node.inner,InnerNode::Symlink(s) if s.link == Path::new("file"))
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
