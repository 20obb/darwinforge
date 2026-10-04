#!/usr/bin/env python3
"""Stand-in ldid: adds an ad-hoc LC_CODE_SIGNATURE to a Mach-O, like ldid -S.

Used only to demo/verify darwinforge's bundle/sign/package stages without an Apple
SDK. This is NOT a real signer; the result carries no valid signature.
"""
import struct
import sys

target = [a for a in sys.argv[1:] if not a.startswith("-")][-1]
data = bytearray(open(target, "rb").read())
assert struct.unpack_from("<I", data, 0)[0] == 0xFEEDFACF, "not a Mach-O"
ncmds, sizeofcmds = struct.unpack_from("<II", data, 16)
at = 32 + sizeofcmds
command = struct.pack("<II", 0x1D, 16) + b"\0" * 8
struct.pack_into("<II", data, 16, ncmds + 1, sizeofcmds + 16)
open(target, "wb").write(bytes(data[:at]) + command + bytes(data[at:]))
print(f"[stub-ldid] ad-hoc signed {target}")