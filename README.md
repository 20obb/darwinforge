# DarwinForge

**Build an installable iOS `.ipa` from C, Objective-C, C++ or Swift — on Linux, on
Windows, on WSL. No Mac. No Xcode. No remote build host. No Apple developer
account.**

[![CI](https://github.com/20obb/darwinforge/actions/workflows/ci.yml/badge.svg)](https://github.com/20obb/darwinforge/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![Dependencies](https://img.shields.io/badge/dependencies-0-success.svg)](Cargo.toml)

**[github.com/20obb/darwinforge](https://github.com/20obb/darwinforge)**

---

## Why this exists

Building an iOS app normally means owning a Mac. Xcode, a provisioning profile,
a developer account, and a Mac-shaped workflow for what is fundamentally a
compile-and-link job. That is a hard requirement for *shipping* to the App Store,
and a completely unnecessary one for the far more common case: producing a
runnable `.ipa` for your own device, a jailbroken phone, AltStore, Sideloadly or
an automated test rig.

DarwinForge closes that gap. It runs the real Apple toolchain — `clang`,
`ld64.lld`, `ldid` — against a real iPhoneOS SDK, on whatever machine you already
have:

```
config → discovery → compile → link → bundle → sign → package
```

It orchestrates tools that already exist rather than reimplementing them. It
does **not** replace Xcode, and it will tell you so: no storyboards, no asset
catalogues, no `.xcodeproj`, and an ad-hoc rather than a certificate-backed
signature. See [LIMITATIONS.md](LIMITATIONS.md) for the full list.

## What makes it different

| | |
| --- | --- |
| **Zero dependencies** | Not one external Rust crate. `cargo build` never touches the network, and the binary has nothing to audit but this repository. |
| **Self-configuring** | `darwinforge bootstrap` detects your distribution, finds the newest toolchain, installs what is missing and fetches an SDK — or tells you precisely why it cannot. |
| **Never hangs** | With no TTY and no `--yes`, every prompt becomes an actionable error instead of a stall. Safe in CI. |
| **Honest** | It reports what it did *not* verify, and a dry run never claims success for work it did not do. |

## Quick start

```sh
git clone https://github.com/20obb/darwinforge.git
cd darwinforge
cargo build --release
./target/release/darwinforge bootstrap     # prepare this machine
./target/release/darwinforge init MyApp
cd MyApp && ../target/release/darwinforge build
```

`bootstrap` is the headline command: it detects your distribution, resolves and
installs the toolchain, fetches an iPhoneOS SDK, and reports whether this
machine can build. See [`darwinforge bootstrap`](#darwinforge-bootstrap).

Already installed a release build? `./install.sh` or `.\install.ps1` puts it on
your `PATH` without root, then offers to bootstrap.

---

## Install

Requires a stable Rust toolchain (1.70+). The crate has **zero external
dependencies**, so `cargo build` never touches the network.

```sh
git clone https://github.com/20obb/darwinforge.git && cd darwinforge
cargo build --release
./target/release/darwinforge --help
```

Every artefact's version is read from `Cargo.toml` at build time and is never
written into a script, so a release and its metadata cannot drift apart.

### Install an end-user build

**Linux and other POSIX systems** — `scripts/install.sh` installs the binary
that sits next to it into `$HOME/.local/bin` (no root, no `sudo`), prints the
line to add to your shell profile if the directory is not already on `PATH`,
and offers to run `bootstrap` afterwards:

```sh
tar xzf darwinforge_<version>_x86_64-unknown-linux-musl.tar.gz
cd darwinforge_<version>_x86_64-unknown-linux-musl
./install.sh                    # add --system for /usr/local/bin
```

| Option | Effect |
| --- | --- |
| `--prefix DIR` | install under `DIR` (default `$HOME/.local`) |
| `--bin DIR` | install exactly into `DIR` |
| `--system` | install into `/usr/local/bin` (may use `sudo`) |
| `--source PATH` | use the binary at `PATH` instead of the bundled one |
| `-y`, `--yes` | never prompt |
| `--no-bootstrap` | do not offer to run `darwinforge bootstrap` |

**Windows** — `scripts/install.ps1` copies `darwinforge.exe` into
`%LOCALAPPDATA%\Programs\darwinforge`, adds that directory to the **user**
`PATH` (so no administrator rights and no UAC prompt), updates the current
process `PATH` so the shell that ran it can use it immediately, and prints
`darwinforge --version` as a check:

```powershell
Expand-Archive darwinforge-x86_64-pc-windows-msvc.zip
cd darwinforge-x86_64-pc-windows-msvc
.\install.ps1                   # add -Bootstrap to run bootstrap now
```

| Parameter | Effect |
| --- | --- |
| `-InstallDir DIR` | where to install (default `%LOCALAPPDATA%\Programs\darwinforge`) |
| `-SourcePath PATH` | use the `.exe` at `PATH` instead of the bundled one |
| `-Bootstrap` | run `darwinforge bootstrap` after installing |
| `-Force` | recreate the install directory instead of merging into it |

Both installers are idempotent: re-running replaces the single binary and adds
a `PATH` entry only if it is not already present.

### Native packages

| Package | Built by | Notes |
| --- | --- | --- |
| `darwinforge_<version>_amd64.deb` | `packaging/deb/build-deb.sh` (via `scripts/release.sh`) | version from `Cargo.toml`; `Depends: git`, `Recommends: clang, lld, zip` |
| `darwinforge-<version>-1*.rpm` | `packaging/rpm/darwinforge.spec` + `rpmbuild` | version injected with `--define "version $v"`; builds from source |
| Arch | `packaging/arch/PKGBUILD` (builds from source) and `packaging/arch/PKGBUILD.binary` (installs the prebuilt tarball) | `pkgver()` reads `Cargo.toml`; no `.pkg.tar.zst` is published |

Releases are cut by pushing a `v*` tag; `.github/workflows/release.yml` builds
the two musl tarballs, the Windows ZIP, the `.deb` and the `.rpm`, then installs
each into a `debian`/`fedora`/`archlinux` container and runs
`packaging/smoke/install-and-run.sh` to prove the binary starts.

### The toolchain

`bootstrap` installs these for you; the table is here so you know what it looks
for, and what to install by hand if you would rather.

| Tool | Why | Required | Debian/Ubuntu | Fedora | Arch | openSUSE | Alpine |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `clang` | compiles C/ObjC/C++ to Mach-O objects | yes | `clang` | `clang` | `clang` | `clang` | `clang` |
| `ld64.lld` | links Mach-O (LLVM's linker) | yes | `lld` | `lld` | `lld` | `lld` | `lld` |
| `ldid` | ad-hoc code signature | yes | `ldid` | *not packaged* | *AUR only* | *not packaged* | *not packaged* |
| `git` | fetches an SDK from the configured repository | yes | `git` | `git` | `git` | `git` | `git` |
| `zip` | *optional*, only for `build.zip = true` | no | `zip` | `zip` | `zip` | `zip` | `zip` |
| `swiftc` | *optional*, only for Swift sources | no | a Swift toolchain | | | | |

There are **no version numbers** in that mapping. Repositories disagree about
names (`lld` is its own package on Debian but ships inside the `llvm` formula on
Homebrew), so `bootstrap` probes the live repository before it names a package,
and reports the truth when there is nothing to install. Where `ldid` is not
packaged at all, the plan names the manager that lacks it rather than printing a
command that cannot work.

Check your machine at any time:

```sh
darwinforge doctor --sdk /path/to/iPhoneOS17.4.sdk
```

`doctor` reports what is missing and prints the exact fix for each item. It
exits non-zero (4) if a required component is absent, so it works as a CI gate.

## `darwinforge bootstrap`

One command prepares a machine. It is **idempotent and safe to re-run**: every
step re-checks the current state, so a second run reports what is already
correct and leaves it alone.

```sh
darwinforge bootstrap
darwinforge setup            # exact alias; one implementation, not two
```

### The six steps

| # | Step | What it does |
| --- | --- | --- |
| 1 | Detect the host | Reads the OS and architecture, then `/etc/os-release`. The distribution is resolved from `ID` **and `ID_LIKE`**, so Ubuntu, Linux Mint, Pop!_OS and Nobara resolve through their lineage even though none is named in the source. The package manager is then *probed* on `PATH`, never assumed. |
| 2 | Resolve the toolchain | Locates `clang`, `ld64.lld`, `ldid`, `git` and `zip` by logical name: a configured path, darwinforge's own managed directory, the plain name on `PATH`, then `<name>-<N>` newest-first, then known LLVM prefixes. Resolved paths are saved to the global config. |
| 3 | Plan and run the installs | For each missing tool, probes the configured repositories and, only if the package really exists, confirms and runs the detected package manager. See [Installing](#installing) below. |
| 4 | Install an iPhoneOS SDK | Already installed and still present? Then this is a no-op. Otherwise it lists the device SDKs the configured repository offers, picks one, and checks it out shallow, blobless and sparse (`git clone --depth 1 --filter=blob:none --sparse`) into the data directory. |
| 5 | Save the global config | Writes the SDK path and version, the resolved tool paths, the source and the state flags to the platform config file. |
| 6 | Summarise | Counts the outcomes, then either prints the next command and exits 0, or lists everything still outstanding and exits 5. |

Steps 1–3 and 5–6 have run. Step 3 has only ever reached the *refusal* path —
see [Installing](#installing). **Step 4 has never completed anywhere** — see
[LIMITATIONS.md](LIMITATIONS.md).

### Installing

Missing tools are installed through the package manager that step 1 detected, so
on Debian-family that is `apt-get install -y …`, on Fedora `dnf install -y …`, on
Arch `pacman -S --needed --noconfirm …`, and so on. Every install command is
non-interactive by construction.

* **Every install is confirmed first.** Installing system packages is the most
  consequential thing `bootstrap` does, so it asks `install packages with
  \`apt-get\`?` before running anything. `--yes` takes the default and logs
  `[auto-yes] install packages with \`apt-get\` (--yes)`. With no TTY and no
  `--yes` it refuses *before spawning any process* and reports the step as
  `[failed]` naming `--yes`.
* **Declining is not a failure.** Say no and the step becomes `[skipped]` with
  *install declined; run the printed command yourself, then re-run bootstrap* —
  a choice, not a broken machine.
* **`sudo` is added only when it is needed**, that is when the manager requires
  elevation (`apt`, `dnf`, `pacman`, `zypper`, `apk`) *and* the process is not
  already root. It is always `sudo -n`, never a bare `sudo`: a password prompt
  must fail fast rather than hang a non-interactive run. Per-user managers
  (`brew`, `scoop`) and an already-root shell get no `sudo` at all.
* **The command is printed as it runs**, and the package manager's own stdout and
  stderr are inherited, so its diagnostics are never swallowed. A non-zero exit
  is reported as `[failed]` with the exit code; a package manager that is not
  found at all says so and tells you to install the tool by hand.
* **Tools are re-resolved afterwards.** An install can put a new `clang` or
  linker on `PATH`, so every tool is looked up again once the installs are done
  and recorded as *found after installing*. Without that pass the summary would
  still list a tool as missing after it had just been installed successfully.

A successful install is tagged `[installed]`; the only other step that can be is
the SDK step.

**This has not been run against a real package manager.** It was developed on
Windows, where there is no `apt`/`dnf`/`pacman` to install from, so the consent
and refusal paths are what have actually been exercised. Treat the Linux
install path as implemented but unproven — [LIMITATIONS.md](LIMITATIONS.md) §1.

### Output

Every step reports one of four tags:

| Tag | Meaning |
| --- | --- |
| `[ok]` | already correct; nothing was changed |
| `[installed]` | something was changed to make it correct |
| `[skipped]` | deliberately not done, with the reason |
| `[failed]` | could not be done, with the reason and what to do |

A real run on Windows (`bootstrap --no-install`, so nothing was changed):

```
DarwinForge bootstrap
===================

host      windows (x86_64)
package   winget (winget)
mode      read-only

[ok] git          using C:\Program Files\Git\cmd\git.EXE
           why: version unknown, found via PathName
[failed] clang        install `LLVM.LLVM`
           `LLVM.LLVM` is not available in the configured winget repositories; refresh the package index and retry, or install it manually and re-run
[failed] ld64.lld     install `LLVM.LLVM`
           `LLVM.LLVM` is not available in the configured winget repositories; …
[failed] ldid         install ldid
           no package for ldid on winget
[failed] zip          install zip
           no package for zip on winget
[skipped] sdk          would install iPhoneOS latest from https://github.com/xybp888/iOS-SDKs
           no SDK is configured; run `darwinforge bootstrap` to install one

summary
-------
  ok        1
  installed 0
  skipped   1
  failed    4

The environment is NOT ready. Outstanding items:
...
next: darwinforge bootstrap            # interactive
      darwinforge bootstrap --dry-run  # show the plan only
error: 5 item(s) still outstanding
  fix: run `darwinforge bootstrap` to finish setting up, or `darwinforge doctor` for details
```

The line under each step is its reason. This run failed every install because
the winget repository index on this machine does not carry those packages. Where
a package *is* available and the run is not read-only, the manager's command is
printed under a `$` line as it executes, and the step ends up `[installed]`,
`[skipped]` or `[failed]`.

### Exit code 5

`bootstrap` exits **5** when the environment is still not usable. That is
deliberately distinct from exit 4 (*a prerequisite is missing* — your `--sdk`
path is wrong), so a CI job can tell *this machine was never set up* apart from
*set up but broken*. A deliberately `[skipped]` step counts as incomplete too:
declining the SDK licence is a choice, and it leaves the machine not ready.

### Options

| Flag | Effect |
| --- | --- |
| `--yes` | accept every default, never prompt. Required in CI. |
| `--no-install` | change nothing: report what is missing and how to fix it |
| `--dry-run` | print the full plan, including the commands that would run, and change nothing |
| `--sdk-version V` \| `latest` | which SDK to install (default `latest`) |
| `--accept-sdk-license` | acknowledge Apple's SDK licence non-interactively |
| `--source URL` | SDK repository; overrides `sdk.source` from the config |

`--dry-run` and `--no-install` share the *same code path* as a real run — only
the point at which a step stops short differs — so a dry run cannot describe
work the real run would not do.

### It never hangs

Every interactive decision goes through one gate. With a TTY it prompts. With
`--yes` it takes the default and prints `[auto-yes] …` so the log still records
what was decided. With **neither**, it refuses immediately with an actionable
error naming the flag that would unblock it — never a prompt nobody will read:

```
error: download an iPhoneOS SDK from a third-party repository needs confirmation,
       but stdin is not a terminal
  fix: run `darwinforge bootstrap --yes` to accept the defaults,
       or pass --accept-sdk-license explicitly
```

Note that `--yes` alone does **not** accept the SDK licence: that question
defaults to *no*, so a fully non-interactive run needs both `--yes` and
`--accept-sdk-license`. Both gates — installing packages and accepting the
licence — go through this same check, so a CI job gets an actionable error in
milliseconds instead of a prompt that will never be read.

### The SDK licence

`bootstrap` is the only part of darwinforge that fetches an SDK, and it fetches
from a **third-party** git repository — not from Apple, and not from
darwinforge. Before the first download it prints this reminder once and asks for
acknowledgement; the answer is remembered in the global config.

> About this SDK: the iPhoneOS SDK you are about to download is Apple's
> software, redistributed by the third-party git repository `<source>`. It is
> not provided by, endorsed by or licensed to you by darwinforge. You are
> responsible for complying with Apple's licence terms for the SDK, and for any
> other rights you need. darwinforge only fetches and uses the files; it never
> modifies or redistributes them itself.

The default source is `https://github.com/xybp888/iOS-SDKs`. It is a *default*,
not a constant baked into the logic: set `sdk.source` in the global config, or
pass `--source <url>`, to use your own mirror. A usable repository has top-level
directories named like `iPhoneOS17.5.sdk`; simulator, watchOS, tvOS and macOS
SDKs are excluded by that prefix alone. Versions are ordered **numerically**, so
`17.10` is newer than `17.5` — the comparison string ordering gets backwards.

If a checkout turns out to hold text stubs where symlinks belong, `bootstrap`
says so immediately rather than letting it fail deep inside clang.

## The SDK

`darwinforge` does not ship or redistribute Apple's SDK, and `build` never
fetches one. Point it at a directory you already have:

```sh
darwinforge build --sdk /opt/ios/iPhoneOS17.4.sdk
# or
export DARWINFORGE_SDK=/opt/ios/iPhoneOS17.4.sdk
```

It must be a **device** SDK. darwinforge validates the layout (`usr/include`,
`usr/lib`, `System/Library/Frameworks`) and reads the version before invoking
anything, trying `SDKSettings.json` first, then `SDKSettings.plist` (XML *and*
the binary `bplist00` format, which a text-only reader cannot parse), and only
then the directory name.

## Where things are kept

| Platform | Global config | Data (installed SDKs, `bin/`, `cache/`) |
| --- | --- | --- |
| Linux / WSL | `$XDG_CONFIG_HOME/darwinforge` or `~/.config/darwinforge` | `$XDG_DATA_HOME/darwinforge` or `~/.local/share/darwinforge` |
| macOS | `~/Library/Application Support/darwinforge` | same |
| Windows | `%APPDATA%\darwinforge` | `%LOCALAPPDATA%\darwinforge` |

`config.toml` lives in the config directory and holds the SDK path and version,
the resolved tool paths, `sdk.source`, `setup_completed` and
`sdk_license_accepted`. It is written atomically through a sibling temp file, so
an interrupted run cannot leave half a config behind, and it is safe to edit by
hand. darwinforge only ever deletes directories inside its own data directory.

## Windows

Two modes exist, and darwinforge will not pretend an unsigned app is a success.

| Mode | When | Notes |
| --- | --- | --- |
| `native` | a Windows `ldid` is available | clang and `ld64.lld` from Windows LLVM, plus that `ldid` |
| `wsl-bridge` | no `ldid`, but WSL is present | the pipeline runs inside WSL, where the toolchain and the SDK's symlinks both work; **recommended on Windows** |

The mode is chosen from what is actually present, not from a setting: `native`
is selected only when a real signer exists. **ldid has no official Windows
build**, so without one darwinforge refuses rather than emitting an `.ipa` that
cannot install. If there is neither a `ldid` nor WSL, it reports `native` so the
failure names the missing pieces rather than the mode.

### Developer Mode is required for the SDK

The SDK is full of symlinks. With `git`'s `core.symlinks` disabled — the default
outside Developer Mode or an elevated shell — a checkout materialises each
symlink as a small text file containing its *target*. The tree then looks
complete while every `.tbd` and header that was a link is garbage, and it fails
much later as an inscrutable compile error. `bootstrap` detects that shape (a
short file whose entire content is a single path) and reports it up front.

Fixes, in the order `bootstrap` prints them:

1. Run `darwinforge bootstrap` **inside WSL**, where symlinks work natively.
2. Turn on Developer Mode (Settings → System → For developers), then
   `git config --global core.symlinks true` and retry.
3. Run one elevated shell so `git` can create the links, then return to normal.

### WSL needs a reboot

`wsl --install` only takes effect after a **REBOOT**. `bootstrap` says so, and
it is safe to re-run afterwards: it re-checks every step and picks up where it
stopped instead of starting over.

## Usage

### Scaffold a project

```sh
darwinforge init Hello
cd Hello
```

Creates `darwinforge.toml`, `Sources/main.m` (a UIKit app that builds its window
in code — no storyboard), a `README.md` and a `.gitignore`.

### Build

```sh
darwinforge build --sdk /path/to/iPhoneOS17.4.sdk          # -> build/Hello.ipa
darwinforge build --sdk ... -o dist/Hello.ipa             # custom output path
darwinforge build --sdk ... --project ../other -v          # verbose: echo every command
```

After a successful `bootstrap` the toolchain is found without any environment
variables, and the installed SDK path is recorded in the global config — but
`build` still needs to be told where that SDK is on this release, because the
config is not consulted for `--sdk` yet. In practice that means either
`--sdk <path>` or `export DARWINFORGE_SDK=<path>`:

```sh
darwinforge build                 # without either, exits 4 with the fix
```

Everything lands under a deterministic `./build/`:

```
build/
├── obj/000-main.o          compiled objects (numbered, so names are stable)
├── Hello                   the linked Mach-O executable
├── Hello.app/              the app bundle
│   ├── Hello
│   ├── Info.plist
│   └── <resources>
└── Hello.ipa               Payload/Hello.app/... zipped
```

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | an external tool failed, or an I/O/format error |
| 2 | bad command line |
| 3 | `darwinforge.toml` is invalid |
| 4 | a prerequisite is missing (tool or SDK path) |
| 5 | the environment is not set up — run `darwinforge bootstrap` |

## Renamed from ipaforge

This tool used to be called `ipaforge`. Everything is `darwinforge` now, but
two old spellings keep working so an existing checkout does not break:

| Old | New | Behaviour |
| --- | --- | --- |
| `ipaforge.toml` | `darwinforge.toml` | if only the old file exists it is still loaded, with a one-line deprecation notice |
| `IPAFORGE_*` env vars | `DARWINFORGE_*` | the old names are still read as a fallback, with a warning |
| `darwinforge setup` | `darwinforge bootstrap` | an exact alias — one implementation, not two |

```sh
# still works, prints a deprecation warning
IPAFORGE_SDK=/opt/ios/iPhoneOS17.4.sdk darwinforge build
```

## `darwinforge.toml`

```toml
[app]
name = "Hello"                  # .app directory name and CFBundleExecutable
bundle_id = "com.example.hello" # CFBundleIdentifier, reverse-DNS
min_ios_version = "13.0"         # MinimumOSVersion and the -target triple
version = "1.0"                 # CFBundleShortVersionString   (optional)
build = "1"                     # CFBundleVersion              (optional)
display_name = "Hello"          # CFBundleDisplayName          (optional)
device_family = [1, 2]          # 1 = iPhone, 2 = iPad         (optional)

[app.info_plist]                # extra Info.plist keys, merged verbatim
# ITSAppUsesNonExemptEncryption = false

[build]
sources = ["Sources/**/*.m"]    # `**` crosses directories
resources = ["Resources/**/*"]  # copied verbatim into the .app
frameworks = ["UIKit", "Foundation"]
# libraries = ["z"]             # -> -lz
# cflags = ["-fobjc-arc"]       # passed to every clang invocation
# ldflags = ["-dead_strip"]     # passed to the linker
# output_dir = "build"          # deterministic output root
# zip = false                   # true = shell out to the system `zip`

[build.swift]                   # only needed for .swift sources
# swift_flags = []
```

Parsing is strict: an unknown key is an error listing the valid keys, not a
silently ignored typo.

Resources are copied to the bundle root so `[NSBundle pathForResource:ofType:]`
finds them. Files inside a `*.lproj` directory keep that directory, because iOS
only looks for localized resources in `<Bundle>.lproj/`.

## What the pipeline does

1. **Discovery** — walks the project, matches `build.sources` /
   `build.resources` globs, and classifies every file. Anything this PoC cannot
   build (storyboards, `.xcassets`, `.xcdatamodeld`, `.xcodeproj`, `.metal`,
   prebuilt `.framework`s) produces a loud `warning:` naming the Apple tool that
   would be needed, and is skipped.
2. **Compile** — `clang -c … -target arm64-apple-ios<min> -isysroot <SDK>
   -arch arm64`, with per-extension flags (`.c` → `-std=gnu11`, `.m` →
   `-fobjc-runtime=ios-13.0`, `.mm`/`.cpp` → `-std=gnu++17`).
3. **Link** — `ld64.lld` (or cctools `ld`) with `-platform_version ios`,
   `-syslibroot`, the framework and library search paths, and `-e _main`. The
   result is then *verified* by parsing its Mach-O header: it must be a 64-bit
   `MH_EXECUTE` for arm64 with a `__TEXT` segment.
4. **Bundle** — generates `Info.plist` (`CFBundleIdentifier`,
   `CFBundleExecutable`, `MinimumOSVersion`, `UIDeviceFamily`,
   `CFBundleSupportedPlatforms`, `UIRequiredDeviceCapabilities`, an empty
   `UILaunchScreen` so iOS does not letterbox a storyboard-less app, …) and
   assembles `Payload/<Name>.app/`.
5. **Sign** — `ldid -S` (ad-hoc / fake signature).
6. **Package** — writes the `.ipa` with a built-in ZIP writer (STORE method,
   explicit POSIX modes so the execute bit survives) or shells out to `zip`.

External tools always inherit stdout/stderr, so their diagnostics are never
swallowed; `-v` additionally echoes each command line before it runs.

## Sample project

`samples/hello-objc` is a complete Objective-C UIKit app: an app delegate that
builds a window and a label in code, plus a JSON resource and an `en.lproj`
`Localizable.strings` file, both copied into the bundle.

```sh
cd samples/hello-objc
darwinforge build --sdk /path/to/iPhoneOS17.4.sdk
```

## Tests

```sh
cargo test
```

* **Unit tests** cover config parsing, plist generation and parsing, glob
  matching, Mach-O header parsing, ZIP CRC-32, and the exact command lines
  handed to clang, the linker and ldid.
* **Integration tests** (`tests/build_ipa.rs`) run the *whole* pipeline against
  a stub toolchain and a synthetic SDK layout, then assert that the `.ipa`
  unzips, `Info.plist` parses and carries the required keys, and the binary is a
  Mach-O arm64 iOS executable with a code signature and the execute bit set.
  The archive is re-read by an independent ZIP reader written in the test.

To exercise the real toolchain, point the test at a genuine SDK:

```sh
DARWINFORGE_TEST_SDK=/path/to/iPhoneOS17.4.sdk cargo test --test build_ipa
```

That test **skips** when the variable is unset, so a green run never silently
implies it was covered.

### Inspecting a build without an SDK

`demo/` contains stub `clang`/`ld64.lld`/`ldid` scripts that emit a
structurally valid (but non-functional) arm64 iOS Mach-O. They exist so you can
exercise the bundle, sign and package stages on any machine:

```sh
export DARWINFORGE_CLANG=$PWD/demo/stub-clang.py     # on Windows: demo/clang.cmd
export DARWINFORGE_LINKER=$PWD/demo/ld64.lld.cmd     # and ld64.lld.cmd
export DARWINFORGE_LDID=$PWD/demo/ldid.cmd           # and ldid.cmd
darwinforge build --sdk /tmp/fake-sdk --project samples/hello-objc -o /tmp/Hello.ipa

python3 demo/verify_ipa.py /tmp/Hello.ipa com.example.hello
```

Create the synthetic SDK first with `python3 demo/make_fake_sdk.py /tmp/fake-sdk`.
The three `demo/*.cmd` wrappers exist so the same commands work in a native
Windows shell without changing the environment-variable syntax.

`verify_ipa.py` re-checks the result using only the Python standard library
(`zipfile` + `plistlib`) plus a hand-written Mach-O header reader — entirely
independent of darwinforge's own code. It is the only check of a produced
`.ipa` that is not darwinforge checking itself.

## What was actually tested

The honest version. This is a proof of concept and the verification is
correspondingly narrow.

**Run by hand on the development machine — Windows 11 (x86_64), Rust 1.99.0
stable:**

* `cargo test` — **270 tests**, all passing (264 unit + 6 integration).
* The **whole pipeline** against the stub toolchain and the synthetic SDK
  layout, on Windows, producing an `.ipa` that `demo/verify_ipa.py` then
  re-validated independently with the Python standard library.
* `bootstrap` in every mode on this Windows host: `--dry-run`, `--no-install`,
  `--yes`, `--yes --accept-sdk-license`, plain (non-interactive), and via the
  `setup` alias — including the exit codes (5 when incomplete, 4 for `doctor`
  and a missing SDK) and the refusal-when-not-a-TTY behaviour.
* `doctor`, and `build` with and without an SDK.
* `scripts/install.sh` (into a throwaway prefix) and `scripts/install.ps1`.
* `shellcheck` (0.11.0) and `sh -n` over every shell script.

**NOT verified — no claim is made about any of these:**

* **No Apple SDK, no real `clang`/`ld64.lld`/`ldid`, no iOS device, ever.** No
  produced `.ipa` has been installed or launched anywhere.
* **`bootstrap` has never completed step 4 and has never installed a package.**
  The install code path exists (`execute_install`) and is unit-tested, but the
  development machine is Windows with no `apt`/`dnf`/`pacman`, so no package has
  ever been installed by darwinforge and no SDK has ever been downloaded. What
  *was* exercised is the consent logic: refusing when there is no TTY and no
  `--yes`, and the read-only paths. See [LIMITATIONS.md](LIMITATIONS.md) §1.
* **The packaging was not built here.** No `.deb`, no `.rpm`, no musl tarball
  and no PKGBUILD package was produced on the development machine: `dpkg-deb`,
  `rpmbuild`, `makepkg` and `docker` are all absent. Those artefacts are built
  and install-tested only by `.github/workflows/release.yml` on Linux runners.
* **No Linux, macOS or WSL machine has run this at all.** Every Linux code path
  — distribution detection, package-manager selection, the `ID_LIKE` lineage,
  `sudo` handling — is unit-tested against *canned* `/etc/os-release` files
  (debian, ubuntu, fedora, arch, manjaro, linuxmint, nobara, alpine, opensuse),
  not executed on those distributions.
* The CI workflow itself (`.github/workflows/ci.yml`, `release.yml`) has not
  been observed running; no build of this tree has been published.

## Contributing

The zero-dependency rule is the defining constraint of this project, so please
keep it: **do not add a crate to `Cargo.toml`.** Everything you need is in `std`,
and if something seems to need a crate, it usually needs a small function
instead.

Before opening a pull request:

```sh
cargo test                                    # 270 tests
cargo clippy --all-targets -- -D warnings     # must be silent
```

Please also read **[LIMITATIONS.md](LIMITATIONS.md)** first. It records what has
and has not been verified, and a change that alters behaviour should update it —
in both directions. If you extend a code path that has never run on a real
machine, say so there rather than letting the docs imply coverage that is not
there.

## Project layout

```
src/            the crate: one module per pipeline stage or subsystem
templates/      main.m, embedded into `darwinforge init` at compile time
samples/        a complete Objective-C UIKit app
demo/           stub toolchain + an independent .ipa verifier, for machines
                that have no Apple SDK
tests/          end-to-end tests against the stub toolchain
packaging/      .deb, .rpm and Arch packaging
scripts/        install + release drivers (no make required)
.github/        ci.yml (test on Linux + Windows) and release.yml (tag a v*)
```

## License

MIT — see [LICENSE](LICENSE).

Apple, iPhone, iOS and `.ipa` are trademarks of Apple Inc. This project is
unaffiliated with Apple. The iPhoneOS SDK is Apple's software and is **not
included, downloaded or redistributed** by this project; `bootstrap` fetches it
from a third-party repository at your request, and you are responsible for
complying with Apple's licence terms for it.