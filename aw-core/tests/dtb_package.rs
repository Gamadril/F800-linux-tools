//! DTB <-> DTS handling of `sunxi-package` items during partition extract / rebuild.
use aw_core::{
    dtb,
    package::Package,
    partition::{self, Kind},
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
            "aw-dtb-tests-{}-{}",
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

const DTS: &str = r#"/dts-v1/;
/ {
	model = "sun8iw21";
	compatible = "allwinner,sun8iw21p1", "allwinner,v837s";
	chosen {
		bootargs = "console=ttyS0 loglevel=8";
	};
	soc {
		uart0: serial@2500000 {
			reg = <0x2500000 0x400>;
			status = "disabled";
		};
	};
};
"#;

/// Build a `sunxi-package` holding `items`, with `slack` spare bytes behind the last item.
fn build_package(items: &[(&str, Vec<u8>)], slack: usize) -> Vec<u8> {
    const HDR: usize = 0x40;
    const ITEM: usize = 0x170;
    let mut out = vec![0u8; HDR + items.len() * ITEM];
    out[..13].copy_from_slice(b"sunxi-package");
    out[0x10..0x14].copy_from_slice(&0x8911_9800u32.to_le_bytes());
    out[0x20..0x24].copy_from_slice(&(items.len() as u32).to_le_bytes());
    out[0x3c..0x40].copy_from_slice(b"MIE;");
    for (i, (name, bytes)) in items.iter().enumerate() {
        let rec = HDR + i * ITEM;
        out[rec..rec + name.len()].copy_from_slice(name.as_bytes());
        let off = out.len() as u32;
        out[rec + 0x40..rec + 0x44].copy_from_slice(&off.to_le_bytes());
        out[rec + 0x44..rec + 0x48].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        out[rec + 0x16c..rec + 0x170].copy_from_slice(b"IIE;");
        out.extend_from_slice(bytes);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out.resize(out.len() + slack, 0);
    let valid = out.len() as u32;
    out[0x24..0x28].copy_from_slice(&valid.to_le_bytes());
    let mut tmp = out.clone();
    tmp[0x14..0x18].copy_from_slice(&0x5F0A_6C39u32.to_le_bytes());
    let sum = tmp.chunks_exact(4).fold(0u32, |a, c| {
        a.wrapping_add(u32::from_le_bytes(c.try_into().unwrap()))
    });
    out[0x14..0x18].copy_from_slice(&sum.to_le_bytes());
    out
}

/// A package with a padded DTB (like `dtc -p`) and an opaque item, extracted into a scratch dir.
fn setup() -> (Scratch, PathBuf, Vec<u8>, Vec<u8>) {
    let mut dtb_bytes = dtb::dts_to_dtb(DTS, None).unwrap();
    let total = dtb_bytes.len() + 256;
    dtb_bytes.resize(total, 0);
    dtb_bytes[4..8].copy_from_slice(&(total as u32).to_be_bytes());
    let raw = build_package(
        &[("optee", vec![0xAA; 64]), ("dtb", dtb_bytes.clone())],
        1024,
    );
    let dir = Scratch::new();
    let file = dir.0.join("package.bin");
    partition::extract(Kind::Package, &raw, &file, false).unwrap();
    (dir, file, raw, dtb_bytes)
}

fn item<'a>(p: &'a Package<'_>, name: &str) -> &'a [u8] {
    p.item_data(p.items.iter().find(|i| i.name == name).unwrap())
}

#[test]
fn extract_writes_dts_only_for_device_trees() {
    let (dir, _, _, dtb_bytes) = setup();
    let out = dir.0.join("package.bin.out");
    assert!(out.join("optee.bin").is_file());
    assert!(!out.join("optee.dts").exists());
    assert_eq!(fs::read(out.join("dtb.bin")).unwrap(), dtb_bytes);
    let dts = fs::read_to_string(out.join("dtb.dts")).unwrap();
    assert_eq!(dts, dtb::dtb_to_dts(&dtb_bytes).unwrap());
    assert!(dts.contains("model = \"sun8iw21\";"));
}

#[test]
fn unmodified_tree_repacks_identically() {
    let (_dir, file, raw, _) = setup();
    assert_eq!(
        partition::rebuild(Kind::Package, &file, raw.clone(), false).unwrap(),
        raw
    );
}

#[test]
fn formatting_only_dts_edit_keeps_the_binary() {
    let (dir, file, raw, _) = setup();
    let path = dir.0.join("package.bin.out/dtb.dts");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("// notes\r\n{}\r\n", text.replace('\n', "\r\n")),
    )
    .unwrap();
    assert_eq!(
        partition::rebuild(Kind::Package, &file, raw.clone(), false).unwrap(),
        raw
    );
}

#[test]
fn edited_dts_is_compiled_into_the_package() {
    let (dir, file, raw, old_dtb) = setup();
    let path = dir.0.join("package.bin.out/dtb.dts");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("loglevel=8", "loglevel=4")
            .replace("disabled", "okay"),
    )
    .unwrap();
    let rebuilt = partition::rebuild(Kind::Package, &file, raw.clone(), false).unwrap();

    assert_eq!(rebuilt.len(), raw.len());
    let pkg = Package::parse(&rebuilt).unwrap(); // checksum is valid again
    let new_dtb = item(&pkg, "dtb");
    assert_ne!(new_dtb, old_dtb.as_slice());
    let tree = dtb::parse_dtb(new_dtb).unwrap();
    let text = dtb::tree_to_dts(&tree);
    assert!(text.contains("loglevel=4") && text.contains("status = \"okay\";"));
    // dtc-style padding of the original DTB is preserved and counted in totalsize
    assert_eq!(
        new_dtb.len(),
        u32::from_be_bytes(new_dtb[4..8].try_into().unwrap()) as usize
    );
    assert!(new_dtb.len() >= 256);
    assert_eq!(item(&pkg, "optee"), [0xAA; 64]);
}

#[test]
fn editing_both_representations_is_a_conflict() {
    let (dir, file, raw, _) = setup();
    let out = dir.0.join("package.bin.out");
    let text = fs::read_to_string(out.join("dtb.dts")).unwrap();
    fs::write(
        out.join("dtb.dts"),
        text.replace("loglevel=8", "loglevel=1"),
    )
    .unwrap();
    let mut bin = fs::read(out.join("dtb.bin")).unwrap();
    bin.push(0);
    fs::write(out.join("dtb.bin"), bin).unwrap();
    let err = partition::rebuild(Kind::Package, &file, raw, false).unwrap_err();
    assert!(err.to_string().contains("edit one representation"), "{err}");
}

#[test]
fn invalid_dts_names_the_file_and_line() {
    let (dir, file, raw, _) = setup();
    let path = dir.0.join("package.bin.out/dtb.dts");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("<0x2500000 0x400>;", "<0x2500000 0x400>"),
    )
    .unwrap();
    let err = partition::rebuild(Kind::Package, &file, raw, false).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("dtb.dts") && msg.contains("line"), "{msg}");
}

#[test]
fn dts_larger_than_the_package_slot_is_rejected() {
    let (dir, file, raw, _) = setup();
    let path = dir.0.join("package.bin.out/dtb.dts");
    let big = format!("big = <{}>;", "1 ".repeat(5000));
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replacen("model", &format!("{big}\n\tmodel"), 1)).unwrap();
    let err = partition::rebuild(Kind::Package, &file, raw, false).unwrap_err();
    assert!(err.to_string().contains("slot only"), "{err}");
}

#[test]
fn dtb_that_cannot_become_dts_still_extracts() {
    let mut tree = dtb::parse_dts(DTS).unwrap();
    tree.root.props.push(dtb::Prop {
        name: "not representable".into(),
        value: vec![1],
    });
    let raw = build_package(&[("dtb", dtb::tree_to_dtb(&tree).unwrap())], 0);
    let dir = Scratch::new();
    let file = dir.0.join("package.bin");
    partition::extract(Kind::Package, &raw, &file, false).unwrap();
    assert!(dir.0.join("package.bin.out/dtb.bin").is_file());
    assert!(!dir.0.join("package.bin.out/dtb.dts").exists());
    assert_eq!(
        partition::rebuild(Kind::Package, &file, raw.clone(), false).unwrap(),
        raw
    );
}
