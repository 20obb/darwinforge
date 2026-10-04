#!/usr/bin/env python3
"""Create the minimal SDK directory layout the stub toolchain expects.

NOT a real SDK: it contains no headers or libraries. It only exists so the
bundle/sign/package stages can be exercised without Apple's SDK.

    python3 demo/make_fake_sdk.py /tmp/fake-sdk
"""
import os
import sys

DIRS = [
    "usr/include",
    "usr/lib",
    "System/Library/Frameworks/UIKit.framework",
    "System/Library/Frameworks/Foundation.framework",
]

SETTINGS = """<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
\t<key>Version</key>
\t<string>17.4</string>
</dict>
</plist>
"""


def main(root):
    for directory in DIRS:
        os.makedirs(os.path.join(root, directory), exist_ok=True)
    with open(os.path.join(root, "SDKSettings.plist"), "w", encoding="utf-8") as handle:
        handle.write(SETTINGS)
    print(f"created fake SDK layout at {root}")
    print("This contains no headers or libraries; it is not usable for compiling.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    main(sys.argv[1])