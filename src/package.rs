//! Stage 6: package `Payload/<Name>.app/` into a `.ipa`.
//!
//! Two backends behind one trait: a dependency-free ZIP writer (STORE method,
//! explicit POSIX modes so the executable bit survives) and the system `zip`
//! command. The built-in writer is the default because it is deterministic and
//! needs nothing installed; `zip = true` in `darwinforge.toml` selects the other.

use std::path::{Path, PathBuf};

use crate::bundle::is_executable_member;
use crate::error::{Error, Result};
use crate::exec;
use crate::reporter::Reporter;

pub trait Packager {
    fn name(&self) -> &'static str;
    /// Write `ipa_path` containing `app_dir` under the `Payload/` prefix.
    fn package(&self, app_dir: &Path, ipa_path: &Path, reporter: &Reporter) -> Result<()>;
}

/// The built-in, dependency-free ZIP writer.
pub struct ZipPackager;

impl Packager for ZipPackager {
    fn name(&self) -> &'static str {
        "built-in zip writer"
    }

    fn package(
        &self,
        app_dir: &Path,
        ipa_path: &Path,
        _reporter: &Reporter,
    ) -> Result<()> {
        let mut entries = Vec::new();
        // iOS requires every .app to sit directly under `Payload/`.
        collect_entries(app_dir, "Payload", &mut entries)?;
        write_zip(&entries, ipa_path)
    }
}

/// Delegate to the system `zip` binary.
pub struct ExternalZipPackager {
    pub zip: PathBuf,
}

impl Packager for ExternalZipPackager {
    fn name(&self) -> &'static str {
        "external zip"
    }

    fn package(&self, app_dir: &Path, ipa_path: &Path, reporter: &Reporter) -> Result<()> {
        let payload_root = app_dir.parent().ok_or_else(|| {
            Error::format("package", format!("{} has no parent directory", app_dir.display()))
        })?;
        // `-y` keeps symlinked dylibs as symlinks, `-X` drops extra file
        // attributes. Store the symlinks relative so the .ipa stays portable.
        let args = vec![
            "-r".to_string(),
            "-q".to_string(),
            "-y".to_string(),
            "-X".to_string(),
            ipa_path.to_string_lossy().to_string(),
            "Payload".to_string(),
        ];
        exec::run(&self.zip.to_string_lossy(), &args, Some(payload_root), reporter)
    }
}

/// One file destined for the archive.
struct Entry {
    /// Path inside the archive, always with `/` separators.
    name: String,
    data: Vec<u8>,
    /// POSIX mode stored in the external attributes.
    mode: u32,
    is_dir: bool,
}

/// Recursively list `root`'s contents, storing names under `archive_root`.
///
/// `archive_root` is `Payload` for an `.ipa`; the bundle directory name is
/// appended automatically, giving `Payload/Hello.app/...`.
///
/// Directory entries carry a trailing `/`, which APPNOTE 4.4.17.1 requires and
/// unarchivers rely on to tell a directory from a same-named empty file.
fn collect_entries(root: &Path, archive_root: &str, entries: &mut Vec<Entry>) -> Result<()> {
    let bundle_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| Error::format("package", format!("{} has no file name", root.display())))?;
    let prefix = format!("{archive_root}/{bundle_name}");
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let read = std::fs::read_dir(&directory).map_err(|source| {
            Error::io(format!("cannot read directory {}", directory.display()), source)
        })?;
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in read {
            let entry = entry.map_err(|source| {
                Error::io(format!("cannot read an entry in {}", directory.display()), source)
            })?;
            children.push(entry.path());
        }
        children.sort();
        for child in children {
            let relative = child
                .strip_prefix(root)
                .map_err(|_| Error::format("package", "path escaped the bundle root"))?;
            let mut name = format!("{prefix}/");
            for component in relative.components() {
                name.push_str(&component.as_os_str().to_string_lossy());
                name.push('/');
            }
            let name = name.trim_end_matches('/').to_string();
            let file_type = std::fs::symlink_metadata(&child)
                .map_err(|source| Error::io(format!("cannot stat {}", child.display()), source))?
                .file_type();
            if file_type.is_symlink() {
                // Follow the link and store its contents: an .ipa must not
                // depend on symlinks surviving the transfer to a device.
                let resolved = std::fs::canonicalize(&child).map_err(|source| {
                    Error::io(format!("cannot resolve symlink {}", child.display()), source)
                })?;
                let data = std::fs::read(&resolved).map_err(|source| {
                    Error::io(format!("cannot read {}", resolved.display()), source)
                })?;
                let executable = is_executable_member(&file_name_of(&child));
                entries.push(Entry { name, data, mode: if executable { 0o755 } else { 0o644 }, is_dir: false });
                continue;
            }
            if file_type.is_dir() {
                // Trailing slash marks this as a directory entry.
                entries.push(Entry {
                    name: format!("{name}/"),
                    data: Vec::new(),
                    mode: 0o755,
                    is_dir: true,
                });
                pending.push(child);
            } else if file_type.is_file() {
                let data = std::fs::read(&child)
                    .map_err(|source| Error::io(format!("cannot read {}", child.display()), source))?;
                let executable = is_executable_member(&file_name_of(&child));
                entries.push(Entry { name, data, mode: if executable { 0o755 } else { 0o644 }, is_dir: false });
            }
        }
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

fn file_name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

/// A fixed MS-DOS timestamp keeps output byte-identical across runs.
fn dos_timestamp() -> u16 {
    // date = ((year-1980) << 9) | (month << 5) | day; 1980-01-01 -> 0x0021.
    // time = (hh << 11) | (mm << 5) | (ss / 2); midnight -> 0.
    0x0021
}

/// Write a ZIP archive containing `entries`, using STORE (no compression).
fn write_zip(entries: &[Entry], ipa_path: &Path) -> Result<()> {
    if let Some(parent) = ipa_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| Error::io(format!("cannot create {}", parent.display()), source))?;
    }
    let mut out: Vec<u8> = Vec::new();
    // Central-directory records: (local header offset, entry, crc).
    let mut directory: Vec<(u32, &Entry, u32)> = Vec::with_capacity(entries.len());
    let timestamp = dos_timestamp();

    for entry in entries {
        let offset = u32::try_from(out.len())
            .map_err(|_| Error::format("package", "archive exceeds the 4 GiB ZIP limit"))?;
        let name = entry.name.as_bytes();
        let crc = crc32(&entry.data);
        let size = u32::try_from(entry.data.len())
            .map_err(|_| Error::format("package", "a bundled file exceeds 4 GiB"))?;
        let name_len = u16::try_from(name.len())
            .map_err(|_| Error::format("package", "a path inside the bundle is too long"))?;

        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: store
        out.extend_from_slice(&timestamp.to_le_bytes());
        out.extend_from_slice(&timestamp.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed size
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        if !entry.is_dir {
            out.extend_from_slice(&entry.data);
        }
        directory.push((offset, entry, crc));
    }

// Central directory.
    let directory_start = u32::try_from(out.len())
        .map_err(|_| Error::format("package", "archive exceeds the 4 GiB ZIP limit"))?;
    for (offset, entry, crc) in &directory {
        let name = entry.name.as_bytes();
        let size = u32::try_from(entry.data.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&0x031eu16.to_le_bytes()); // version made by: UNIX, spec 3.0
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: store
        out.extend_from_slice(&timestamp.to_le_bytes());
        out.extend_from_slice(&timestamp.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        // High 16 bits carry the Unix mode, low bits the MS-DOS attributes.
        out.extend_from_slice(
            &(entry.mode << 16 | if entry.is_dir { 0x10 } else { 0 }).to_le_bytes(),
        );
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name);
    }
    let directory_size = u32::try_from(out.len()).unwrap_or(u32::MAX) - directory_start;

    // End of central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // central directory disk
    out.extend_from_slice(&(directory.len() as u16).to_le_bytes());
    out.extend_from_slice(&(directory.len() as u16).to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    std::fs::write(ipa_path, &out)
        .map_err(|source| Error::io(format!("cannot write {}", ipa_path.display()), source))
}

/// CRC-32 (IEEE), the checksum ZIP requires.
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 != 0 { 0xedb8_8320 ^ (value >> 1) } else { value >> 1 };
        }
        *entry = value;
    }
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc = table[((crc ^ *byte as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

/// Pick a packager according to the config.
pub fn select(
    config_uses_external_zip: bool,
    zip_binary: Option<PathBuf>,
) -> Result<Box<dyn Packager>> {
    if !config_uses_external_zip {
        return Ok(Box::new(ZipPackager));
    }
    let zip = zip_binary.ok_or_else(|| Error::Prereq {
        what: "build.zip = true but no `zip` command was found".to_string(),
        fix: "install Info-ZIP (`apt install zip`), or remove `zip = true` from \
              darwinforge.toml to use the built-in writer"
            .to_string(),
    })?;
    Ok(Box::new(ExternalZipPackager { zip }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn external_zip_selection_errors_when_missing() {
        let error = match select(true, None) {
            Ok(_) => panic!("must fail when zip is requested but absent"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("build.zip"));
    }

    #[test]
    fn default_is_builtin() {
        assert_eq!(select(false, None).expect("ok").name(), "built-in zip writer");
    }
}