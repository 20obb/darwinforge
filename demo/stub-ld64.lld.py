#!/usr/bin/env python3
"""Stand-in ld64.lld: writes a minimal arm64 iOS MH_EXECUTE Mach-O.

Used only to demo/verify darwinforge's bundle/sign/package stages without an Apple
SDK. This is NOT a linker and produces a binary that will not run on a device.
"""
import struct
import sys

args = sys.argv[1:]
out = args[args.index("-o") + 1]
cmds = [(0x19, 72), (0x32, 24)]  # LC_SEGMENT_64, LC_BUILD_VERSION
sizeofcmds = 96
header = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, len(cmds), sizeofcmds, 0x00200085, 0)
seg = struct.pack("<II", 0x19, 72)
seg += b"__TEXT".ljust(16, b"\0")
seg += struct.pack("<4Q", 0, 0x1000, 0, 0x1000)
seg += struct.pack("<4I", 7, 5, 0, 0)
build = struct.pack("<6I", 0x32, 24, 2, 0x000D0000, 0x00110000, 0)
open(out, "wb").write(header + seg + build)
print(f"[stub-ld64.lld] wrote executable {out}")