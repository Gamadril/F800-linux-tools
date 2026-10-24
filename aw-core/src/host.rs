//! Host filesystem output with no-clobber defaults.
use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn safe_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains(['/', '\\', '\0', '\t', '\n', '\r'])
        || name == "."
        || name == ".."
    {
        bail!("unsafe or empty filename: {name:?}");
    }
    #[cfg(windows)]
    windows_name(name)?;
    Ok(())
}

#[cfg(any(windows, test))]
fn windows_name(name: &str) -> Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.contains([':', '*', '?', '"', '<', '>', '|'])
        || name.ends_with(['.', ' '])
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        bail!("filename cannot be represented on Windows: {name:?}");
    }
    Ok(())
}

pub fn relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\\', '\0', '\t', '\n', '\r'])
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe relative path: {path:?}");
    }
    for component in path.split('/') {
        safe_name(component)?;
    }
    Ok(())
}

/// Store filesystem paths with '/' on every host, matching the firmware manifest.
pub fn manifest_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("not a relative firmware path");
        };
        parts.push(name.to_str().context("non-UTF8 firmware path")?);
    }
    Ok(parts.join("/"))
}

pub fn new_file_mode(metadata: &fs::Metadata) -> u16 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        (metadata.permissions().mode() & 0o7777) as u16
    }
    #[cfg(not(unix))]
    {
        if metadata.is_dir() {
            0o755
        } else {
            0o644
        }
    }
}

pub fn make_editable(path: &Path, mode: u16) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode as u32 | 0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

/// Reject symlink ancestors even when their target is inside the extraction tree.
pub fn contained_path(root: &Path, relative: &str) -> Result<PathBuf> {
    relative_path(relative)?;
    let mut p = root.to_path_buf();
    for c in Path::new(relative).components() {
        p.push(c);
        if let Ok(md) = fs::symlink_metadata(&p) {
            if md.file_type().is_symlink() {
                bail!("symlink in output path: {}", p.display());
            }
        }
    }
    Ok(p)
}

fn unused_path(destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = destination
        .file_name()
        .context("output has no filename")?
        .to_string_lossy();
    Ok(parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}

pub fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!(
            "output already exists: {} (choose a new destination)",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Commit a complete file atomically without replacing an existing destination.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    require_absent(path)?;
    let temp = unused_path(path)?;
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::hard_link(&temp, path).with_context(|| format!("commit {}", path.display()))?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    result
}

/// An extraction is published only after every nested parser succeeds.
pub struct OutputDir {
    staging: PathBuf,
    destination: PathBuf,
    committed: bool,
}
impl OutputDir {
    pub fn new(destination: &Path) -> Result<Self> {
        require_absent(destination)?;
        let staging = unused_path(destination)?;
        fs::create_dir(&staging)?;
        Ok(Self {
            staging,
            destination: destination.to_path_buf(),
            committed: false,
        })
    }
    pub fn path(&self) -> &Path {
        &self.staging
    }
    pub fn commit(mut self) -> Result<()> {
        require_absent(&self.destination)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let source = std::ffi::CString::new(self.staging.as_os_str().as_bytes())?;
            let destination = std::ffi::CString::new(self.destination.as_os_str().as_bytes())?;
            // CString owns valid NUL-terminated paths throughout this syscall.
            let result = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(target_os = "linux"))]
        fs::rename(&self.staging, &self.destination)?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for OutputDir {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.staging);
        }
    }
}

#[cfg(test)]
mod portability_tests {
    use super::*;
    #[test]
    fn windows_names_and_manifest_paths() {
        for name in [
            "CON",
            "nul.txt",
            "COM1",
            "Lpt9",
            "file:stream",
            "bad?name",
            "trailing.",
            "space ",
        ] {
            assert!(windows_name(name).is_err());
        }
        for name in ["console", "busybox", "manifest.tsv", "COM10"] {
            assert!(windows_name(name).is_ok());
        }
        assert_eq!(
            manifest_path(&Path::new("usr").join("bin").join("busybox")).unwrap(),
            "usr/bin/busybox"
        );
        assert!(manifest_path(Path::new("../escape")).is_err());
    }
}
