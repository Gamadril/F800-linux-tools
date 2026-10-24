use aw_core::{
    bootimg::BootImg,
    dump,
    env::Env,
    host,
    imagewty::{Image, Item},
    layout::{Layout, Part},
    partition::{self, Kind},
    squash,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "aw-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn programmer_dump_roundtrip_and_isolated_logo_edit() {
    let dir = Scratch::new();
    let original = include_bytes!("../../dumps/EN25QH128_20261018_23412.BIN");
    let input = dir.0.join("original.bin");
    fs::write(&input, original).unwrap();
    let out = dir.0.join("tree");
    dump::extract_dump(&input, &out, false).unwrap();
    assert_eq!(
        dump::pack_dump(&out, false).unwrap().full_image.as_slice(),
        original
    );
    assert!(dump::extract_dump(&input, &out, false).is_err());
    let path = out.join("bootlogo.jpg");
    let jpeg = fs::read(&path).unwrap();
    let mut changed = jpeg[..2].to_vec();
    changed.extend_from_slice(b"\xff\xfe\x00\x06TEST");
    changed.extend_from_slice(&jpeg[2..]);
    fs::write(path, &changed).unwrap();
    let packed = dump::pack_dump(&out, false).unwrap();
    let logo = packed.layout.get("bootlogo").unwrap();
    assert_eq!(
        &packed.full_image[..logo.offset as usize],
        &original[..logo.offset as usize]
    );
    assert_ne!(packed.full_image.as_slice(), original);
    assert_eq!(
        aw_core::bootlogo::extract_jpeg(&packed.partitions["bootlogo"]).unwrap(),
        changed
    );
    // A conflicting raw edit must not be silently overwritten by nested files.
    let mut raw_logo = fs::read(out.join("bootlogo.bin")).unwrap();
    let last = raw_logo.len() - 1;
    raw_logo[last] ^= 1;
    fs::write(out.join("bootlogo.bin"), raw_logo).unwrap();
    assert!(dump::pack_dump(&out, false).is_err());
}

#[test]
fn layout_rejects_overlaps_duplicates_and_bad_paths_and_preserves_gaps() {
    for name in ["../escape", "/absolute", "..", "env"] {
        let l = Layout {
            flash_size: 32,
            parts: vec![
                Part {
                    name: "env".into(),
                    offset: 0,
                    size: 8,
                },
                Part {
                    name: name.into(),
                    offset: 8,
                    size: 8,
                },
            ],
        };
        assert!(l.validate().is_err());
    }
    let mut l = Layout {
        flash_size: 32,
        parts: vec![Part {
            name: "opaque".into(),
            offset: 8,
            size: 8,
        }],
    };
    assert!(l.validate().is_ok());
    let dir = Scratch::new();
    fs::write(dir.0.join("layout.json"), serde_json::to_vec(&l).unwrap()).unwrap();
    fs::write(dir.0.join("opaque.bin"), [7; 8]).unwrap();
    let template: Vec<_> = (0..32).collect();
    fs::write(dir.0.join(dump::TEMPLATE_FILE), &template).unwrap();
    let p = dump::pack_dump(&dir.0, false).unwrap();
    assert_eq!(&p.full_image[..8], &template[..8]);
    assert_eq!(&p.full_image[16..], &template[16..]);
    l.parts.push(Part {
        name: "overlap".into(),
        offset: 9,
        size: 1,
    });
    assert!(l.validate().is_err());
    assert!(Layout::from_dump(&[0; 32], 0x80000).is_err());
}

#[test]
fn env_edit_preserves_all_header_variants() {
    for (flags, crc_start) in [(None, 4), (Some(0), 4), (Some(7), 5)] {
        let mut env = Env::from_text("hello=world\n");
        env.flags = flags;
        env.crc_start = crc_start;
        let raw = env.build(1024).unwrap();
        let parsed = Env::parse(&raw).unwrap();
        assert_eq!(parsed.flags, flags);
        assert_eq!(parsed.crc_start, crc_start);
        let dir = Scratch::new();
        let path = dir.0.join("env.bin");
        fs::write(dir.0.join("env.txt"), "hello=changed\n").unwrap();
        let bytes = partition::rebuild(Kind::Env, &path, raw, false).unwrap();
        let parsed = Env::parse(&bytes).unwrap();
        assert_eq!(parsed.get("hello"), Some("changed"));
        assert_eq!((parsed.flags, parsed.crc_start), (flags, crc_start));
    }
}

#[test]
fn boot_rebuild_includes_ramdisk_and_second_stage() {
    let mut header = vec![0; 0x800];
    header[..8].copy_from_slice(b"ANDROID!");
    header[0x24..0x28].copy_from_slice(&0x800u32.to_le_bytes());
    let boot = BootImg {
        header,
        kernel: vec![1; 5],
        ramdisk: vec![2; 7],
        second: vec![3; 9],
    };
    let raw = partition::pad(boot.build().unwrap(), 0x4000).unwrap();
    let dir = Scratch::new();
    let file = dir.0.join("boot.fex");
    partition::extract(Kind::Boot, &raw, &file, false).unwrap();
    fs::write(dir.0.join("boot.fex.out/ramdisk.img"), [4; 11]).unwrap();
    fs::write(dir.0.join("boot.fex.out/second.img"), [5; 13]).unwrap();
    let rebuilt = partition::rebuild(Kind::Boot, &file, raw, false).unwrap();
    let boot = BootImg::parse(&rebuilt).unwrap();
    assert_eq!(boot.kernel, [1; 5]);
    assert_eq!(boot.ramdisk, [4; 11]);
    assert_eq!(boot.second, [5; 13]);
}

#[test]
fn image_preserves_header_and_item_metadata_and_rejects_corruption() {
    let image = Image::new(vec![Item::new(
        "TEST",
        "RFSFAT16",
        "test.fex",
        vec![42; 1030],
    )]);
    let bytes = image.build(true).unwrap();
    let (mut parsed, ok) = Image::parse(&bytes).unwrap();
    assert_eq!(ok, Some(true));
    parsed.header[0x24..0x34].copy_from_slice(&[7; 16]); // PID/VID/hardware/firmware IDs
    parsed.header[0x200] = 0x9a;
    parsed.items[0].record[0x180] = 0x66;
    let bytes = parsed.build(true).unwrap();
    let (again, _) = Image::parse(&bytes).unwrap();
    assert_eq!(again.header, parsed.header);
    assert_eq!(again.items[0].record, parsed.items[0].record);
    let mut bad = bytes.clone();
    bad[2048] ^= 1;
    assert!(Image::parse(&bad).is_err());
    let mut bad = image.build(false).unwrap();
    bad[1024 + 0x134..1024 + 0x13c].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Image::parse(&bad).is_err());
    for n in [0, 8, 32, 1000, 1030] {
        assert!(Image::parse(&bytes[..n]).is_err());
    }
}

#[test]
fn no_clobber_outputs_and_traversal_rejection() {
    let dir = Scratch::new();
    let file = dir.0.join("existing");
    host::write_new(&file, b"original").unwrap();
    assert!(host::write_new(&file, b"overwrite").is_err());
    assert_eq!(fs::read(&file).unwrap(), b"original");
    assert!(host::contained_path(&dir.0, "../escape").is_err());
    #[cfg(windows)]
    for name in ["CON", "NUL.txt", "file:stream", "LPT1", "trailing."] {
        assert!(host::safe_name(name).is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&dir.0, dir.0.join("link")).unwrap();
        assert!(host::contained_path(&dir.0, "link/escape").is_err());
    }
    let dest = dir.0.join("uncommitted");
    {
        let temp = host::OutputDir::new(&dest).unwrap();
        fs::write(temp.path().join("partial"), b"data").unwrap();
    }
    assert!(!dest.exists());
}

#[test]
fn squashfs_edits_preserve_root_metadata_options_and_special_nodes() {
    use backhand::compression::Compressor;
    use backhand::{
        FilesystemCompressor, FilesystemReader, FilesystemWriter, InnerNode, NodeHeader,
    };
    use std::io::{BufReader, Cursor};
    let mut w = FilesystemWriter::default();
    w.set_time(1234);
    w.set_root_mode(0o750);
    w.set_root_uid(17);
    w.set_root_gid(23);
    w.set_compressor(FilesystemCompressor::new(Compressor::Xz, None).unwrap());
    w.push_dir("/owners", NodeHeader::new(0o755, 17, 23, 123))
        .unwrap();
    w.push_file(
        Cursor::new(b"before"),
        "/file",
        NodeHeader::new(0o640, 41, 42, 123),
    )
    .unwrap();
    w.push_char_device(0x123, "/device", NodeHeader::new(0o600, 0, 0, 124))
        .unwrap();
    w.push_symlink("file", "/link", NodeHeader::new(0o777, 0, 0, 125))
        .unwrap();
    let mut bytes = Cursor::new(Vec::new());
    w.write(&mut bytes).unwrap();
    let original = bytes.into_inner();
    let before =
        FilesystemReader::from_reader(BufReader::new(Cursor::new(original.clone()))).unwrap();
    let dir = Scratch::new();
    let tree = dir.0.join("rootfs");
    squash::extract(&original, &tree).unwrap();
    assert!(!squash::tree_modified(&tree).unwrap());
    fs::write(tree.join("file"), b"after").unwrap();
    assert!(squash::tree_modified(&tree).unwrap());
    let rebuilt = squash::build(&tree).unwrap();
    let after = FilesystemReader::from_reader(BufReader::new(Cursor::new(rebuilt))).unwrap();
    let before_root = before
        .files()
        .find(|n| n.fullpath == std::path::Path::new("/"))
        .unwrap();
    let after_root = after
        .files()
        .find(|n| n.fullpath == std::path::Path::new("/"))
        .unwrap();
    assert_eq!(before_root.header, after_root.header);
    assert_eq!(
        format!("{:?}", before.compression_options),
        format!("{:?}", after.compression_options)
    );
    assert!(after
        .files()
        .any(|n| matches!(n.inner, InnerNode::CharacterDevice(_))));
    assert!(after.files().any(
        |n| matches!(&n.inner,InnerNode::Symlink(link) if link.link == std::path::Path::new("file"))
    ));
    #[cfg(windows)]
    assert!(!tree.join("link").exists());
    let manifest = fs::read_to_string(tree.join(squash::MANIFEST)).unwrap();
    fs::write(
        tree.join(squash::MANIFEST),
        manifest.replace("\t640\t41\t42\t", "\t600\t43\t44\t"),
    )
    .unwrap();
    assert!(squash::tree_modified(&tree).unwrap());
}

#[test]
fn invalid_egon_and_boot_headers_return_errors() {
    let mut egon = [0; 48];
    egon[4..12].copy_from_slice(b"eGON.BT0");
    assert!(aw_core::egon::verify(&egon).is_err());
    let mut boot = vec![0; 0x800];
    boot[..8].copy_from_slice(b"ANDROID!");
    boot[0x24..0x28].copy_from_slice(&0x80000000u32.to_le_bytes());
    assert!(BootImg::parse(&boot).is_err());
}
