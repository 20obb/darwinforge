#!/usr/bin/env python3
"""Stand-in clang: writes a real arm64 iOS Mach-O object at the -o path.

Used only to demo/verify darwinforge's pipeline on a machine with no Apple SDK.
This is NOT a compiler and produces nothing runnable.
"""
import struct
import sys

args = sys.argv[1:]
out = args[args.index("-o") + 1]
# A minimal but structurally valid 64-bit arm64 Mach-O object (MH_OBJECT).
cmds = [(0x19, 72, "__TEXT")]
header = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 1, 72, 72, 0x00200085, 0)
seg = struct.pack("<II", 0x19, 72)
seg += b"__TEXT".ljust(16, b"\0")
seg += struct.pack("<4Q", 0, 0x1000, 0, 0x1000)
seg += struct.pack("<4I", 7, 5, 0, 0)
open(out, "wb").write(header + seg)
print(f"[stub-clang] wrote object {out}")