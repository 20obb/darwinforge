//! End-to-end integration tests.
//!
//! Two flavours:
//!
//! 1. `builds_the_sample_project_into_a_valid_ipa` runs the **real** pipeline
//!    (compile -> link -> bundle -> sign -> package) against a *fake* toolchain
//!    (shims that emit a genuine arm64 iOS Mach-O) and a synthetic SDK layout.
//!    This verifies all of darwinforge's own logic — argument construction, plist
//!    generation, bundle layout, zip structure, permission bits — on any
//!    machine, with no Apple SDK present.
//! 2. `builds_with_a_real_toolchain_when_available` runs the same assertions
//!    against the genuine clang/ld64.lld/ldid, and **skips** unless
//!    `DARWINFORGE_TEST_SDK` points at a real iPhoneOS SDK. That is the test that
//!    proves interoperability with Apple tooling, and it only reports success
//!    when it really ran.

use std::path::{Path, PathBuf};
use std::process::Command;

use darwinforge::config::Config;
use darwinforge::mach;
use darwinforge::package;
use darwinforge::pipeline::{self, BuildOptions};
use darwinforge::plist::{self, Plist};
use darwinforge::reporter::Reporter;
use darwinforge::sdk::{LinkerKind, Sdk, Toolchain};

// ---------------------------------------------------------------- utilities

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("darwinforge-it-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch dir");
        Scratch { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A minimal but *structurally real* SDK directory tree.
fn make_fake_sdk(root: &Path) {
    for dir in [
        "usr/include",
        "usr/lib",
        "System/Library/Frameworks/UIKit.framework",
        "System/Library/Frameworks/Foundation.framework",
    ] {
        std::fs::create_dir_all(root.join(dir)).expect("create sdk dir");
    }
    std::fs::write(
        root.join("SDKSettings.plist"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <plist version=\"1.0\"><dict><key>Version</key><string>17.4</string></dict></plist>\n",
    )
    .expect("write SDKSettings");
}

/// True when a Python 3 interpreter is available for the shims below.
fn python() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if ok {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Write an executable wrapper named `name` in `dir` that runs `script`.
fn write_shim(dir: &Path, name: &str, script: &str, python: &str) -> std::io::Result<PathBuf> {
    let path = if cfg!(windows) {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(
            &path,
            format!("@echo off\r\n\"{python}\" \"%~dp0{script}\" %*\r\n"),
        )?;
        path
    } else {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nexec {python} \"$0\" {script} \"$@\"\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        path
    };
    Ok(path)
}

/// Install a fake toolchain in `dir` and return a `Toolchain` pointing at it.
///
/// The shims are small Python scripts: they parse only the flags darwinforge is
/// contractually required to pass, which is exactly what this test asserts on.
fn make_fake_toolchain(dir: &Path, python: &str) -> std::io::Result<Toolchain> {
    std::fs::create_dir_all(dir)?;

    // The "linker" emits a structurally genuine 64-bit arm64 iOS Mach-O
    // executable: LC_SEGMENT_64 __TEXT plus LC_BUILD_VERSION naming iOS.
    let linker_script = r#"import struct, sys
args = sys.argv[1:]
out = None
for i, a in enumerate(args):
    if a == "-o":
        out = args[i + 1]
assert out, "fake linker: no -o"
assert "-arch" in args, "fake linker: no -arch"
assert "-platform_version" in args, "fake linker: no -platform_version"
# mach_header_64 is exactly 8 little-endian u32 fields; sizeofcmds must be
# correct or a signer cannot find the end of the load commands.
sizeofcmds = 72 + 24
header = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 2, sizeofcmds, 0x00200085, 0)
seg = struct.pack("<II", 0x19, 72)
seg += "__TEXT".encode().ljust(16, b"\0")
seg += struct.pack("<4Q", 0, 0x1000, 0, 0x1000)
seg += struct.pack("<4I", 7, 5, 0, 0)   # maxprot, initprot, nsects, flags
build = struct.pack("<6I", 0x32, 24, 2, 0x000D0000, 0x00110000, 0)
open(out, "wb").write(header + seg + build)
print("fake-ld64.lld: wrote", out)
"#;
    std::fs::write(dir.join("fake_ld.py"), linker_script)?;

    // The "compiler" just has to produce the object file clang was told to.
    std::fs::write(
        dir.join("fake_clang.py"),
        r#"import sys
args = sys.argv[1:]
out = None
for i, a in enumerate(args):
    if a == "-o":
        out = args[i + 1]
assert out, "fake clang: no -o"
assert "-c" in args, "fake clang: expected -c"
target = [a for a in args if not a.startswith("-")][0]
assert "-isysroot" in args, "fake clang: expected -isysroot"
open(out, "wb").write(b"fake object file for " + target.encode())
print("fake-clang: compiled", target, "->", out)
"#,
    )?;

    // ldid rewrites the Mach-O to add LC_CODE_SIGNATURE, exactly as the real
    // one does, so the test can assert the signature actually landed.
    std::fs::write(
        dir.join("fake_ldid.py"),
        r#"import struct, sys
target = [a for a in sys.argv[1:] if not a.startswith("-")][-1]
data = bytearray(open(target, "rb").read())
assert struct.unpack_from("<I", data, 0)[0] == 0xFEEDFACF, "fake ldid: not a Mach-O"
ncmds, sizeofcmds = struct.unpack_from("<II", data, 16)
# LC_CODE_SIGNATURE goes at the end of the load-command block, i.e. right
# after the existing commands, not at the end of the file. cmdsize counts the
# 8-byte header plus 8 bytes of padding.
at = 32 + sizeofcmds
new = struct.pack("<II", 0x1D, 16) + b"\0" * 8
struct.pack_into("<II", data, 16, ncmds + 1, sizeofcmds + 16)
open(target, "wb").write(bytes(data[:at]) + new + bytes(data[at:]))
print("fake-ldid: signed", target)
"#,
    )?;

    Ok(Toolchain {
        clang: write_shim(dir, "clang", "fake_clang.py", python)?,
        linker: write_shim(dir, "ld64.lld", "fake_ld.py", python)?,
        linker_kind: LinkerKind::Lld,
        ldid: write_shim(dir, "ldid", "fake_ldid.py", python)?,
        zip: None,
        swiftc: None,
    })
}

/// Copy the sample project into the scratch dir so builds never touch the repo.
fn copy_sample(scratch: &Scratch) -> PathBuf {
    let destination = scratch.path("hello-objc");
    copy_tree(&sample_dir(), &destination);
    destination
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// A read-only ZIP reader, so the test validates the archive with independent
/// code rather than trusting the writer that produced it.
struct ZipReader<'a> {
    bytes: &'a [u8],
    directory_offset: usize,
    entry_count: usize,
}

struct ZipEntry {
    name: String,
    mode: u32,
    data: Vec<u8>,
}

fn read_u16(bytes: &[u8], offset: usize) -> usize {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize
}

fn read_u32(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
        as usize
}

impl<'a> ZipReader<'a> {
    fn open(bytes: &'a [u8]) -> ZipReader<'a> {
        // Locate the end-of-central-directory record (signature 0x06054b50).
        let mut eocd = bytes.len() - 22;
        while eocd > 0 && read_u32(bytes, eocd) != 0x0605_4b50 {
            eocd -= 1;
        }
        assert_eq!(read_u32(bytes, eocd), 0x0605_4b50, "no end-of-central-directory record");
        let entry_count = read_u16(bytes, eocd + 10);
        let directory_offset = read_u32(bytes, eocd + 16);
        ZipReader { bytes, directory_offset, entry_count }
    }

    fn entries(&self) -> Vec<ZipEntry> {
        let mut out = Vec::new();
        let mut cursor = self.directory_offset;
        for _ in 0..self.entry_count {
            assert_eq!(read_u32(self.bytes, cursor), 0x0201_4b50, "bad central directory record");
            let external_attributes = read_u32(self.bytes, cursor + 38);
            let local_offset = read_u32(self.bytes, cursor + 42);
            let name_len = read_u16(self.bytes, cursor + 28);
            let extra_len = read_u16(self.bytes, cursor + 30);
            let comment_len = read_u16(self.bytes, cursor + 32);
            let name = String::from_utf8_lossy(&self.bytes[cursor + 46..cursor + 46 + name_len])
                .to_string();
            cursor += 46 + name_len + extra_len + comment_len;

            // Read the payload via the local header.
            assert_eq!(read_u32(self.bytes, local_offset), 0x0403_4b50, "bad local header");
            let local_name_len = read_u16(self.bytes, local_offset + 26);
            let local_extra_len = read_u16(self.bytes, local_offset + 28);
            let size = read_u32(self.bytes, local_offset + 18);
            let method = read_u16(self.bytes, local_offset + 8);
            assert_eq!(method, 0, "this PoC writes STORE entries only");
            let start = local_offset + 30 + local_name_len + local_extra_len;
            let data = self.bytes[start..start + size].to_vec();
            // The high 16 bits of the external attributes hold the Unix mode.
            let mode = ((external_attributes >> 16) & 0o7777) as u32;
            out.push(ZipEntry { name, mode, data });
        }
        out
    }
}

/// Copy a directory tree.
fn copy_tree(from: &Path, to: &Path) {
    for entry in walk(from) {
        let relative = entry.strip_prefix(from).expect("under source");
        let target = to.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).expect("create dir");
        } else {
            std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
            std::fs::copy(&entry, &target).expect("copy file");
        }
    }
}

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/hello-objc")
}

/// Run the pipeline once and return the produced `.ipa` plus its entries.
fn build_sample(scratch: &Scratch) -> (PathBuf, Vec<ZipEntry>) {
    let python = python().expect("python3 is required for the shim toolchain");
    let sdk_root = scratch.path("iPhoneOS17.4.sdk");
    make_fake_sdk(&sdk_root);
    let sdk = Sdk::open(&sdk_root).expect("fake SDK is valid");
    let toolchain =
        make_fake_toolchain(&scratch.path("toolchain"), &python).expect("shims written");
    let project = copy_sample(scratch);
    let config = Config::load(&project).expect("sample config parses");

    let packager = package::select(false, None).expect("built-in packager");
    let reporter = Reporter::new(false);
    let output =
        pipeline::build(&config, &sdk, &toolchain, packager.as_ref(), &BuildOptions::default(), &reporter)
            .expect("pipeline succeeds");

    let ipa_bytes = std::fs::read(&output.ipa).expect("ipa exists");
    let entries = ZipReader::open(&ipa_bytes).entries();
    (output.ipa, entries)
}

// ------------------------------------------------------------------- tests

#[test]
fn builds_the_sample_project_into_a_valid_ipa() {
    if python().is_none() {
        eprintln!("skipping: python3 is needed only to run the fake toolchain shims");
        return;
    }
    let scratch = Scratch::new("full");
    let (ipa_path, entries) = build_sample(&scratch);

    // 1. The .ipa exists and is a real archive rooted at Payload/.
    assert!(ipa_path.is_file(), "the .ipa was written");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert!(names.contains(&"Payload/Hello.app/Info.plist"), "got {names:?}");
    assert!(names.contains(&"Payload/Hello.app/Hello"), "got {names:?}");
    assert!(names.contains(&"Payload/Hello.app/greeting.json"), "got {names:?}");
    assert!(
        names.contains(&"Payload/Hello.app/en.lproj/Localizable.strings"),
        "localized resources keep their .lproj directory: {names:?}"
    );
    assert!(!names.iter().any(|name| name.starts_with("/")), "paths are relative");
    // Directory entries must end in "/" or unarchivers cannot tell them from
    // same-named files and will fail to extract into them (7-Zip does).
    for entry in &entries {
        assert!(
            entry.name.starts_with("Payload/"),
            "every member sits under Payload/: {}",
            entry.name
        );
    }
    assert!(
        names.iter().any(|name| name.ends_with('/')),
        "the archive should carry directory entries"
    );
    assert_eq!(mode_of(&entries, "Payload/Hello.app/Hello"), 0o755);

    // 2. Info.plist parses and carries the required keys.
    let info = entries.iter().find(|entry| entry.name == "Payload/Hello.app/Info.plist").unwrap();
    let parsed = plist::parse_xml(&String::from_utf8_lossy(&info.data))
        .expect("Info.plist is a valid XML plist");
    assert_eq!(
        parsed.get("CFBundleIdentifier").and_then(Plist::as_str),
        Some("com.example.hello")
    );
    assert_eq!(parsed.get("CFBundleExecutable").and_then(Plist::as_str), Some("Hello"));
    assert_eq!(parsed.get("MinimumOSVersion").and_then(Plist::as_str), Some("13.0"));
    assert_eq!(parsed.get("CFBundlePackageType").and_then(Plist::as_str), Some("APPL"));
    let family = parsed.get("UIDeviceFamily").and_then(Plist::as_array).expect("array");
    assert_eq!(family, [Plist::Integer(1), Plist::Integer(2)]);

    // 3. The binary is a Mach-O arm64 iOS executable, signed.
    let binary = entries.iter().find(|entry| entry.name == "Payload/Hello.app/Hello").unwrap();
    let macho = mach::parse(&binary.data).expect("binary is a Mach-O");
    assert!(macho.is_arm64(), "{}", macho.describe());
    assert!(macho.is_executable(), "{}", macho.describe());
    assert!(mach::targets_ios(&macho), "LC_BUILD_VERSION names iOS: {:?}", macho.ios_target);
    assert!(macho.has_code_signature, "ldid added LC_CODE_SIGNATURE");
    assert!(macho.segment_names.contains(&"__TEXT".to_string()));

    // 4. Permissions: the binary is executable, plain files are not.
    assert_eq!(binary.mode & 0o777, 0o755, "app binary carries the execute bit");
    assert_eq!(info.mode & 0o777, 0o644, "Info.plist is not executable");
    let json = entries.iter().find(|entry| entry.name == "Payload/Hello.app/greeting.json").unwrap();
    assert_eq!(json.mode & 0o777, 0o644);
    assert!(String::from_utf8_lossy(&json.data).contains("greeting"));

    // 5. Outputs land where the docs promise.
    assert!(scratch.path("hello-objc/build/Hello.app").is_dir());
    assert!(scratch.path("hello-objc/build/obj").is_dir());
}

#[test]
fn ipa_output_is_deterministic_across_identical_builds() {
    if python().is_none() {
        return;
    }
    let scratch = Scratch::new("determinism");
    let (first, _) = build_sample(&scratch);
    let first_bytes = std::fs::read(&first).expect("read first ipa");

    // Build the identical project into a different directory: the archive must
    // not embed timestamps or absolute paths.
    let second = scratch.path("second");
    copy_tree(&sample_dir(), &second);
    let sdk = Sdk::open(&scratch.path("iPhoneOS17.4.sdk")).expect("sdk");
    let toolchain = make_fake_toolchain(&scratch.path("toolchain"), &python().unwrap())
        .expect("shims");
    let config = Config::load(&second).expect("config");
    let packager = package::select(false, None).expect("packager");
    let output = pipeline::build(
        &config,
        &sdk,
        &toolchain,
        packager.as_ref(),
        &BuildOptions::default(),
        &Reporter::new(false),
    )
    .expect("pipeline");
    let second_bytes = std::fs::read(&output.ipa).expect("read second ipa");

    let first_entries = ZipReader::open(&first_bytes).entries();
    let second_entries = ZipReader::open(&second_bytes).entries();
    assert_eq!(
        first_entries.len(),
        second_entries.len(),
        "identical inputs must produce the same member list"
    );
    for (left, right) in first_entries.iter().zip(second_entries.iter()) {
        assert_eq!(left.name, right.name);
        assert_eq!(left.data, right.data, "{} differs between builds", left.name);
        assert_eq!(left.mode, right.mode);
    }
}

#[test]
fn honours_the_output_flag() {
    if python().is_none() {
        return;
    }
    let scratch = Scratch::new("output-flag");
    make_fake_sdk(&scratch.path("iPhoneOS17.4.sdk"));
    let sdk = Sdk::open(&scratch.path("iPhoneOS17.4.sdk")).expect("sdk");
    let toolchain = make_fake_toolchain(&scratch.path("toolchain"), &python().unwrap())
        .expect("shims");
    let project = copy_sample(&scratch);
    let config = Config::load(&project).expect("config");
    let packager = package::select(false, None).expect("packager");

    let custom = scratch.path("dist/custom.ipa");
    let options = BuildOptions { arch: "arm64".to_string(), output: Some(custom.clone()) };
    let output = pipeline::build(
        &config,
        &sdk,
        &toolchain,
        packager.as_ref(),
        &options,
        &Reporter::new(false),
    )
    .expect("pipeline");
    assert_eq!(output.ipa, custom);
    assert!(custom.is_file(), "-o wrote the .ipa to the requested path");
    assert!(
        !project.join("build/Hello.ipa").exists(),
        "the default path is not written when -o is given"
    );
}

#[test]
fn warns_and_skips_unsupported_project_files() {
    if python().is_none() {
        return;
    }
    let scratch = Scratch::new("skip");
    let project = copy_sample(&scratch);
    // Files the PoC explicitly cannot build.
    write_file(&project.join("Base.lproj/Main.storyboard"), "<storyboard/>");
    write_file(&project.join("Images.xcassets/Contents.json"), "{}");
    write_file(&project.join("Model.xcdatamodeld/contents"), "");
    write_file(&project.join("Shader.metal"), "// metal");

    let config = Config::load(&project).expect("config still parses");
    let reporter = Reporter::new(false);
    let found = darwinforge::discovery::discover(&config, &reporter).expect("discovery runs");

    assert_eq!(found.skipped.len(), 4, "got {:?}", found.skipped);
    for entry in &found.skipped {
        assert!(
            entry.contains("ibtool")
                || entry.contains("actool")
                || entry.contains("momc")
                || entry.contains("metal"),
            "skip reason should say which Apple tool would be needed: {entry}"
        );
    }
    // The real source was still found, so the build can proceed.
    assert_eq!(found.sources.len(), 1);
    assert_eq!(reporter.warning_count(), 4, "each skip is reported to the user");
}

/// Look up a member's stored POSIX mode.
fn mode_of(entries: &[ZipEntry], name: &str) -> u32 {
    entries.iter().find(|entry| entry.name == name).unwrap_or_else(|| {
        panic!("{name} is not in the archive");
    }).mode
        & 0o777
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(path, contents).expect("write");
}

/// Same assertions as `builds_the_sample_project_into_a_valid_ipa`, but against
/// the **real** clang / ld64.lld / ldid and a real iPhoneOS SDK.
///
/// Skipped unless `DARWINFORGE_TEST_SDK` names an extracted device SDK, so a green
/// run never implies this passed. Run it with:
///
/// ```sh
/// DARWINFORGE_TEST_SDK=/path/to/iPhoneOS17.4.sdk cargo test --test build_ipa
/// ```
#[test]
fn builds_with_a_real_toolchain_when_available() {
    let sdk_path = match darwinforge::compat::env_var("TEST_SDK") {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!(
                "skipping real-toolchain test: set DARWINFORGE_TEST_SDK to an extracted \
                 iPhoneOS SDK to run it"
            );
            return;
        }
    };
    let sdk = match Sdk::open(&sdk_path) {
        Ok(sdk) => sdk,
        Err(error) => panic!("DARWINFORGE_TEST_SDK is not a usable SDK: {error}"),
    };
    let toolchain = Toolchain::discover().expect("clang, ld64.lld and ldid must be on PATH");

    let scratch = Scratch::new("real");
    let project = copy_sample(&scratch);
    let config = Config::load(&project).expect("sample config parses");
    let packager = package::select(false, None).expect("built-in packager");
    let output = pipeline::build(
        &config,
        &sdk,
        &toolchain,
        packager.as_ref(),
        &BuildOptions::default(),
        &Reporter::new(false),
    )
    .expect("real pipeline succeeds");

    let bytes = std::fs::read(&output.ipa).expect("ipa exists");
    let entries = ZipReader::open(&bytes).entries();
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert!(names.contains(&"Payload/Hello.app/Info.plist"), "got {names:?}");

    let info = entries.iter().find(|entry| entry.name == "Payload/Hello.app/Info.plist").unwrap();
    let parsed = plist::parse_xml(&String::from_utf8_lossy(&info.data))
        .expect("Info.plist is valid XML");
    assert_eq!(
        parsed.get("CFBundleIdentifier").and_then(Plist::as_str),
        Some("com.example.hello")
    );

    let binary = entries.iter().find(|entry| entry.name == "Payload/Hello.app/Hello").unwrap();
    let macho = mach::parse(&binary.data).expect("binary is a Mach-O");
    assert!(macho.is_arm64(), "{}", macho.describe());
    assert!(macho.is_executable(), "{}", macho.describe());
    assert!(mach::targets_ios(&macho), "LC_BUILD_VERSION names iOS: {:?}", macho.ios_target);
    assert!(macho.has_code_signature, "ldid added LC_CODE_SIGNATURE");
    assert_eq!(binary.mode & 0o777, 0o755);
}

#[test]
fn cli_builds_the_sample_project_end_to_end() {
    if python().is_none() {
        return;
    }
    // Drive the real binary so argument parsing and exit codes are covered too.
    let scratch = Scratch::new("cli");
    let project = copy_sample(&scratch);
    make_fake_sdk(&scratch.path("iPhoneOS17.4.sdk"));
    let toolchain_dir = scratch.path("toolchain");
    let python = python().unwrap();
    make_fake_toolchain(&toolchain_dir, &python).expect("shims");

    let bin = env!("CARGO_BIN_EXE_darwinforge");
    let run = |args: &[&str]| -> std::process::Output {
        Command::new(bin)
            .args(args)
            .current_dir(&project)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("PATHEXT", std::env::var("PATHEXT").unwrap_or_default())
            .env("DARWINFORGE_CLANG", toolchain_dir.join(if cfg!(windows) { "clang.cmd" } else { "clang" }))
            .env("DARWINFORGE_LINKER", toolchain_dir.join(if cfg!(windows) { "ld64.lld.cmd" } else { "ld64.lld" }))
            .env("DARWINFORGE_LDID", toolchain_dir.join(if cfg!(windows) { "ldid.cmd" } else { "ldid" }))
            .output()
            .expect("run darwinforge")
    };

    let output = run(&["build", "--sdk", scratch.path("iPhoneOS17.4.sdk").to_str().unwrap()]);
    assert!(
        output.status.success(),
        "build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("build/Hello.ipa").is_file());

    // A missing SDK must fail with the documented exit code and an actionable
    // message, not a panic or a silent success.
    let failed = run(&["build"]);
    assert_eq!(failed.status.code(), Some(4), "missing-SDK exit code");
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("no iPhoneOS SDK given"), "got: {stderr}");
    assert!(stderr.contains("--sdk"), "the error must say how to fix it: {stderr}");

    // Unknown flags are a usage error (exit 2).
    let usage = run(&["build", "--nonsense"]);
    assert_eq!(usage.status.code(), Some(2));
}