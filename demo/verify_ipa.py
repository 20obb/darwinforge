#!/usr/bin/env python3
"""Validate a built .ipa using only the Python standard library.

Independent of darwinforge's own code: `plistlib` parses Info.plist, and the
Mach-O header is checked field by field against <mach-o/loader.h>.

    python3 demo/verify_ipa.py path/to/App.ipa [expected.bundle.id]
"""
import plistlib
import struct
import sys
import zipfile

MH_MAGIC_64 = 0xFEEDFACF
CPU_TYPE_ARM64 = 0x0100000C
MH_EXECUTE = 2
LC_SEGMENT_64 = 0x19
LC_BUILD_VERSION = 0x32
LC_CODE_SIGNATURE = 0x1D
PLATFORM_IOS = 2


def fail(message):
    print(f"FAIL: {message}")
    sys.exit(1)


def check_ipa(path, expected_bundle_id=None):
    with zipfile.ZipFile(path) as archive:
        broken = archive.testzip()
        if broken is not None:
            fail(f"corrupt member: {broken}")
        names = archive.namelist()
        print(f"archive: {len(names)} member(s)")
        for name in names:
            print(f"  {name}")

        if not any(n.startswith("Payload/") for n in names):
            fail("no member under Payload/ — iOS will not recognise this as an .ipa")

        info_names = [n for n in names if n.endswith(".app/Info.plist")]
        if len(info_names) != 1:
            fail(f"expected exactly one Payload/<App>.app/Info.plist, found {info_names}")
        info_name = info_names[0]
        app_dir = info_name.rsplit("/", 1)[0]

        # --- Info.plist, parsed by plistlib -------------------------------
        plist = plistlib.loads(archive.read(info_name))
        print(f"\nInfo.plist ({info_name}):")

        identifier = plist.get("CFBundleIdentifier")
        if not identifier or "." not in identifier:
            fail(f"CFBundleIdentifier is {identifier!r}, expected reverse-DNS")
        if expected_bundle_id and identifier != expected_bundle_id:
            fail(f"CFBundleIdentifier is {identifier!r}, expected {expected_bundle_id!r}")
        print(f"  ok  CFBundleIdentifier = {identifier}")

        executable = plist.get("CFBundleExecutable")
        if not executable:
            fail("CFBundleExecutable is missing")
        print(f"  ok  CFBundleExecutable = {executable}")

        if plist.get("CFBundlePackageType") != "APPL":
            fail(f"CFBundlePackageType is {plist.get('CFBundlePackageType')!r}, expected 'APPL'")
        print("  ok  CFBundlePackageType = APPL")

        if not plist.get("MinimumOSVersion"):
            fail("MinimumOSVersion is missing")
        print(f"  ok  MinimumOSVersion = {plist['MinimumOSVersion']}")

        supported = plist.get("CFBundleSupportedPlatforms")
        if supported != ["iPhoneOS"]:
            fail(f"CFBundleSupportedPlatforms is {supported!r}, expected ['iPhoneOS']")
        print("  ok  CFBundleSupportedPlatforms = ['iPhoneOS']")

        family = plist.get("UIDeviceFamily")
        if not isinstance(family, list) or not family or not set(family) <= {1, 2}:
            fail(f"UIDeviceFamily is {family!r}, expected a non-empty subset of [1, 2]")
        print(f"  ok  UIDeviceFamily = {family}")

        # --- the executable ------------------------------------------------
        binary_name = f"{app_dir}/{executable}"
        if binary_name not in names:
            fail(f"CFBundleExecutable {executable!r} does not match any bundle member")
        data = archive.read(binary_name)
        print(f"\nexecutable ({binary_name}): {len(data)} bytes")

        (magic, cputype, cpusubtype, filetype, ncmds, sizeofcmds, flags, reserved) = \
            struct.unpack_from("<8I", data, 0)
        if magic != MH_MAGIC_64:
            fail(f"magic is 0x{magic:08x}, expected 0x{MH_MAGIC_64:08x} (64-bit Mach-O)")
        if cputype != CPU_TYPE_ARM64:
            fail(f"cputype is 0x{cputype:x}, expected 0x{CPU_TYPE_ARM64:x} (arm64)")
        if filetype != MH_EXECUTE:
            fail(f"filetype is {filetype}, expected {MH_EXECUTE} (MH_EXECUTE)")
        print("  ok  magic=0xfeedfacf cputype=arm64 filetype=MH_EXECUTE")

        offset = 32
        segments, ios_version, signed = [], None, False
        for _ in range(ncmds):
            cmd, cmdsize = struct.unpack_from("<II", data, offset)
            if cmdsize < 8 or offset + cmdsize > len(data):
                fail(f"malformed load command at byte {offset} (cmdsize {cmdsize})")
            if cmd == LC_SEGMENT_64:
                segments.append(data[offset + 8:offset + 24].split(b"\0")[0].decode())
            elif cmd == LC_BUILD_VERSION:
                platform, minos, sdk, _ = struct.unpack_from("<4I", data, offset + 8)
                if platform != PLATFORM_IOS:
                    fail(f"LC_BUILD_VERSION platform is {platform}, expected {PLATFORM_IOS} (iOS)")
                ios_version = (minos >> 16, minos & 0xFFFF, sdk >> 16, sdk & 0xFFFF)
            elif cmd == LC_CODE_SIGNATURE:
                signed = True
            offset += cmdsize

        if "__TEXT" not in segments:
            fail(f"no __TEXT segment (found {segments})")
        print(f"  ok  segments = {segments}")
        if ios_version is None:
            fail("no LC_BUILD_VERSION: the binary does not declare an iOS target")
        print(f"  ok  iOS min {ios_version[0]}.{ios_version[1]}, sdk {ios_version[2]}.{ios_version[3]}")
        if not signed:
            fail("no LC_CODE_SIGNATURE: ldid did not sign the binary")
        print("  ok  LC_CODE_SIGNATURE present (ad-hoc signed)")

    print("\nPASS: the .ipa unzips, Info.plist is valid, and the binary is a "
          "Mach-O arm64 iOS executable.")


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        print(__doc__)
        sys.exit(2)
    check_ipa(sys.argv[1], sys.argv[2] if len(sys.argv) == 3 else None)