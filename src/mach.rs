//! Minimal Mach-O header reader.
//!
//! Used to *verify* what the linker produced: that the executable really is a
//! 64-bit Mach-O for arm64, that it is an executable (`MH_EXECUTE`), that it
//! targets iOS (via `LC_BUILD_VERSION`) and that `ldid` added a code signature.
//! Also used by the integration test instead of shelling out to `file`.

pub const MH_MAGIC_64: u32 = 0xfeed_facf;
pub const MH_CIGAM_64: u32 = 0xcffa_edfe;
pub const FAT_MAGIC: u32 = 0xcafe_babe;
pub const FAT_CIGAM: u32 = 0xbeba_feca;

pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
pub const CPU_SUBTYPE_ARM64_ALL: u32 = 0;

pub const MH_EXECUTE: u32 = 0x2;
pub const MH_DYLIB: u32 = 0x6;

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_BUILD_VERSION: u32 = 0x32;
pub const LC_CODE_SIGNATURE: u32 = 0x1d;
pub const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;

/// `PLATFORM_IOS` from `<mach-o/loader.h>`.
pub const PLATFORM_IOS: u32 = 2;

#[derive(Debug, Clone)]
pub struct MachO {
    pub cputype: u32,
    pub cpusubtype: u32,
    pub filetype: u32,
    pub ncmds: u32,
    /// `true` when an `LC_BUILD_VERSION`/`LC_VERSION_MIN_IPHONEOS` names iOS.
    pub ios_target: Option<String>,
    pub has_code_signature: bool,
    pub segment_names: Vec<String>,
}

impl MachO {
    pub fn is_arm64(&self) -> bool {
        self.cputype == CPU_TYPE_ARM64
    }

    pub fn is_executable(&self) -> bool {
        self.filetype == MH_EXECUTE
    }

    /// `Mach-O 64-bit executable arm64` style summary, like `file` output.
    pub fn describe(&self) -> String {
        let arch = match self.cputype {
            CPU_TYPE_ARM64 => "arm64".to_string(),
            0x0100_0007 => "x86_64".to_string(),
            0x0000_000c => "arm".to_string(),
            other => format!("cpu 0x{other:x}"),
        };
        let kind = match self.filetype {
            MH_EXECUTE => "executable".to_string(),
            MH_DYLIB => "dylib".to_string(),
            other => format!("filetype {other}"),
        };
        format!("Mach-O 64-bit {kind} {arch}")
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| format!("truncated Mach-O at byte {offset}"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parse the 64-bit Mach-O header and walk its load commands.
pub fn parse(bytes: &[u8]) -> Result<MachO, String> {
    if bytes.len() < 32 {
        return Err("file is too small to be a Mach-O binary".to_string());
    }
    let magic = read_u32(bytes, 0)?;
    match magic {
        MH_MAGIC_64 => {}
        MH_CIGAM_64 => {
            return Err("Mach-O is big-endian, which iOS never uses".to_string())
        }
        FAT_MAGIC | FAT_CIGAM => {
            return Err(
                "this is a fat/universal binary; darwinforge expects a thin arm64 \
                 slice (link with -arch arm64)"
                    .to_string(),
            )
        }
        other => {
            return Err(format!(
                "not a Mach-O binary: magic is 0x{other:08x} (a Mach-O starts \
                 with 0xfeedfacf)"
            ))
        }
    }

    let mut macho = MachO {
        cputype: read_u32(bytes, 4)?,
        cpusubtype: read_u32(bytes, 8)?,
        filetype: read_u32(bytes, 12)?,
        ncmds: read_u32(bytes, 16)?,
        ios_target: None,
        has_code_signature: false,
        segment_names: Vec::new(),
    };

    let mut offset = 32usize;
    for _ in 0..macho.ncmds {
        let cmd = read_u32(bytes, offset)?;
        let cmdsize = read_u32(bytes, offset + 4)? as usize;
        if cmdsize < 8 || offset + cmdsize > bytes.len() {
            return Err(format!("malformed load command at byte {offset}"));
        }
        match cmd {
            LC_SEGMENT_64 => {
                let name: String = bytes
                    .get(offset + 8..offset + 24)
                    .ok_or_else(|| "truncated LC_SEGMENT_64".to_string())?
                    .iter()
                    .take_while(|byte| **byte != 0)
                    .map(|byte| *byte as char)
                    .collect();
                macho.segment_names.push(name);
            }
            LC_BUILD_VERSION => {
                let platform = read_u32(bytes, offset + 8)?;
                let minos = read_u32(bytes, offset + 16)?;
                let sdk = read_u32(bytes, offset + 20)?;
                macho.ios_target = Some(format!(
                    "platform {} minos {}.{} sdk {}.{}",
                    platform_name(platform),
                    minos >> 16,
                    minos & 0xffff,
                    sdk >> 16,
                    sdk & 0xffff
                ));
            }
            LC_VERSION_MIN_IPHONEOS => {
                let version = read_u32(bytes, offset + 8)?;
                macho.ios_target = Some(format!(
                    "iOS version {}.{}",
                    version >> 16,
                    version & 0xffff
                ));
            }
            LC_CODE_SIGNATURE => macho.has_code_signature = true,
            _ => {}
        }
        offset += cmdsize;
    }
    Ok(macho)
}

fn platform_name(platform: u32) -> String {
    match platform {
        1 => "macOS",
        2 => "iOS",
        3 => "tvOS",
        4 => "watchOS",
        6 => "macCatalyst",
        7 => "iOSSimulator",
        _ => "unknown",
    }
    .to_string()
}

/// True when a load command names iOS as the target platform.
pub fn targets_ios(macho: &MachO) -> bool {
    macho.ios_target.as_deref().map(|text| text.contains("iOS")).unwrap_or(false)
}

/// Build a minimal arm64 executable carrying the given load commands.
///
/// Used by the test suite (unit + integration) to exercise header parsing and
/// to stand in for a real linker when the full iOS toolchain is unavailable.
pub mod testkit {
    use super::*;

    pub fn synthesize(ncmds: &[u32], platform: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
        bytes.extend_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
        bytes.extend_from_slice(&CPU_SUBTYPE_ARM64_ALL.to_le_bytes());
        bytes.extend_from_slice(&MH_EXECUTE.to_le_bytes());
        bytes.extend_from_slice(&(ncmds.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // sizeofcmds
        bytes.extend_from_slice(&0x0020_0085u32.to_le_bytes()); // flags
        bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved
        // mach_header_64 is exactly 8 fields = 32 bytes.
        for cmd in ncmds {
            let cmdsize = if *cmd == LC_SEGMENT_64 { 72 } else { 24 };
            bytes.extend_from_slice(&cmd.to_le_bytes());
            bytes.extend_from_slice(&(cmdsize as u32).to_le_bytes());
            match *cmd {
                LC_SEGMENT_64 => {
                    // segname is exactly 16 bytes, NUL-padded.
                    let mut segname = [0u8; 16];
                    segname[..6].copy_from_slice(b"__TEXT");
                    bytes.extend_from_slice(&segname);
                    bytes.extend_from_slice(&0u64.to_le_bytes());
                    bytes.extend_from_slice(&4096u64.to_le_bytes());
                    bytes.extend_from_slice(&0u64.to_le_bytes());
                    bytes.extend_from_slice(&4096u64.to_le_bytes());
                    bytes.extend_from_slice(&7u32.to_le_bytes()); // maxprot
                    bytes.extend_from_slice(&5u32.to_le_bytes()); // initprot
                    bytes.extend_from_slice(&0u32.to_le_bytes()); // nsects
                    bytes.extend_from_slice(&0u32.to_le_bytes()); // flags
                }
                LC_BUILD_VERSION => {
                    let (plat, minos, sdk) =
                        platform.unwrap_or((PLATFORM_IOS, 0x000d_0000, 0x0011_0000));
                    bytes.extend_from_slice(&plat.to_le_bytes());
                    bytes.extend_from_slice(&minos.to_le_bytes());
                    bytes.extend_from_slice(&sdk.to_le_bytes());
                    bytes.extend_from_slice(&0u32.to_le_bytes()); // ntools
                }
                _ => bytes.extend_from_slice(&[0u8; 16]),
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::synthesize;
    use super::*;

    #[test]
    fn parses_arm64_executable() {
        let bytes = synthesize(&[LC_SEGMENT_64, LC_BUILD_VERSION], None);
        let macho = parse(&bytes).expect("parses");
        assert!(macho.is_arm64());
        assert!(macho.is_executable());
        assert!(targets_ios(&macho));
        assert_eq!(macho.segment_names, vec!["__TEXT"]);
        assert_eq!(macho.describe(), "Mach-O 64-bit executable arm64");
        assert!(!macho.has_code_signature);
    }

    #[test]
    fn detects_code_signature() {
        let bytes = synthesize(&[LC_SEGMENT_64, LC_BUILD_VERSION, LC_CODE_SIGNATURE], None);
        assert!(parse(&bytes).expect("parses").has_code_signature);
    }

    #[test]
    fn rejects_non_macho() {
        assert!(parse(b"#!/bin/sh\nnot a macho at all........").is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn rejects_fat_binary() {
        let mut bytes = FAT_MAGIC.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        assert!(parse(&bytes).unwrap_err().contains("fat/universal"));
    }

    #[test]
    fn rejects_wrong_architecture() {
        let mut bytes = synthesize(&[LC_SEGMENT_64, LC_BUILD_VERSION], None);
        bytes[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes()); // x86_64
        let macho = parse(&bytes).expect("parses");
        assert!(!macho.is_arm64());
        assert_eq!(macho.describe(), "Mach-O 64-bit executable x86_64");
    }

    #[test]
    fn detects_legacy_version_min_command() {
        let bytes = synthesize(&[LC_SEGMENT_64, LC_VERSION_MIN_IPHONEOS], None);
        let macho = parse(&bytes).expect("parses");
        assert!(targets_ios(&macho), "LC_VERSION_MIN_IPHONEOS also means iOS");
    }
}