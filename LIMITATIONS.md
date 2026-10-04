# LIMITATIONS

What `darwinforge` does **not** do, and what has **not** been verified.

Read this before trusting an `.ipa` it produces.

---

## 1. Not verified on real Apple tooling or a device

This is the most important section.

The development machine (Windows 11, x86_64) had **no Apple SDK, no
`ld64.lld` and no `ldid`**, and no iOS device. Everything that could be
verified without them was verified; everything else is written from the
documented behaviour of those tools and is **unverified**.

| Area | Status |
| --- | --- |
| Config parsing, plist generation, glob matching, Mach-O header parsing, ZIP writing, CLI parsing and exit codes | **Executed and tested** (270 tests) |
| Full pipeline wiring (compile → link → bundle → sign → package), argument construction, archive layout, permission bits | **Executed end to end against stub tools** on Windows; the stubs assert the same flags darwinforge promises to pass |
| `bootstrap` host detection, package planning, prompt modes, exit code 5 | **Executed by hand** on Windows, in every flag combination |
| `bootstrap` installing packages from a **real package manager** | **Code path written and unit-tested, never executed.** `execute_install` spawns the detected manager, but the development machine is Windows with no `apt`/`dnf`/`pacman`, so no package has ever been installed by darwinforge. Only the refusal paths (no TTY, no `--yes`) and the declined path have been executed here. |
| `bootstrap` **downloading an SDK** (step 4) | **Never executed.** No SDK has ever been fetched by this tool. |
| Distribution detection and package-manager selection | **Unit-tested against canned `/etc/os-release` files** (debian, ubuntu, fedora, arch, manjaro, linuxmint, nobara, alpine, opensuse). **Not run on any of those distributions.** |
| The `.deb`, the `.rpm`, the musl tarballs, the PKGBUILDs and the Windows ZIP | **Never built on the development machine** — `dpkg-deb`, `rpmbuild`, `makepkg` and `docker` are all absent there. Built only by the GitHub Actions workflow on Linux runners. |
| Any Linux, macOS or WSL machine running this at all | **None.** Every such code path has only been exercised through unit tests on Windows. |
| Compilation of real Objective-C against a real SDK headers | **Not executed.** The clang flags are written from `clang(1)`/`ld(1)` documentation. |
| Linking against real `.tbd` stubs with `ld64.lld` | **Not executed.** Flag names (`-platform_version`, `-syslibroot`, `-no_data_in_code_info`) are from `ld64.lld` docs and cctools. |
| `ldid -S` on a real Mach-O | **Not executed.** The stub adds `LC_CODE_SIGNATURE` the same way, but real `ldid` also writes a signature blob and may need a writable, correctly-aligned binary. |
| Installation on a physical iPhone/iPad | **Never tested.** No claim is made that any produced `.ipa` installs or launches. |

### The install path has never run against a real package manager

`bootstrap` does install packages: it resolves the toolchain, probes the
repositories, confirms, and then runs the detected manager (`execute_install` in
`src/bootstrap.rs`). A successful install is reported as `[installed]`, a
non-zero exit as `[failed]` with the manager's own diagnostics, and the tools are
re-resolved afterwards so the summary reflects what is really on `PATH` now.

**None of that has ever been executed for real.** The development machine is
Windows 11, which has no `apt`, `dnf`, `pacman`, `zypper` or `apk`; the only
managers reachable there are `winget`, `choco` and `scoop`. So:

* No package has ever been installed by darwinforge. `winget list` on the
  development machine reports no `LLVM.LLVM`, and `clang`, `ld64.lld` and `ldid`
  are still absent after a full `bootstrap` run.
* What *has* been executed is the refusal and consent logic: with no TTY and no
  `--yes`, the install is refused before any process is spawned, and the step
  reports `[failed]` naming `--yes`. Both were observed on the development
  machine, and are unit-tested.
* The `sudo -n` construction, the declined-install `[skipped]` wording and the
  re-resolution pass are unit-tested against synthetic inputs, not exercised
  against a live `apt-get`.

So treat "bootstrap installs the toolchain" as **implemented but unproven on
Linux**. If it misbehaves there — a wrong package name, a `sudo` that fails, a
manager that prompts where it should not — nothing in this repository would have
caught it.

### `build` does not read the global config

`bootstrap` records the installed SDK path in `~/.config/darwinforge/config.toml`,
but `build` and `doctor` do not consult that file: they use `--sdk` and
`DARWINFORGE_SDK` only. A bare `darwinforge build` after a successful bootstrap
therefore exits 4 with *no iPhoneOS SDK given*, and the installed SDK is not
picked up automatically.

### Windows

The Developer Mode and WSL guidance is printed on Windows and unit-tested, but
the **WSL bridge itself has never been run** — no WSL distribution was available
on the development machine. `bootstrap` refuses to check out an SDK on native
Windows and says why, which is correct behaviour, but it means the WSL path is
the only Windows route to a working SDK and it is **untested**. The same applies
to the "no Windows `ldid`, use the WSL bridge" decision: the logic and its
message are unit-tested, not exercised end to end.

**The integration test `builds_with_a_real_toolchain_when_available` exists
precisely to close this gap.** It runs the same assertions against real
`clang`/`ld64.lld`/`ldid` and a real SDK, and skips unless you opt in:

```sh
DARWINFORGE_TEST_SDK=/path/to/iPhoneOS17.4.sdk cargo test --test build_ipa
```

If that test has not been run on your machine, treat the linker and compiler
integration as untested.

### Likely first-run problems with a real SDK

Expect to have to adjust:

* **Objective-C runtime linking.** `libobjc.tbd` lives in `<SDK>/usr/lib` and is
  found via `-L`. If your SDK ships the runtime only as
  `usr/lib/libobjc.A.tbd`, add `libraries = ["objc"]` to `darwinforge.toml`.
* **`-no_data_in_code_info`.** Harmless on modern lld/cctools, but if your
  linker rejects it, remove it from `src/link.rs` or override via `ldflags`.
* **Bitcode / older SDKs.** SDKs older than Xcode 14 may require `-bitcode`
  handling. Not addressed.
* **`.tbd` support.** `ld64.lld` reads Apple `.tbd` v2/v3 stub libraries; very
  old SDKs used `.tbd` v1, which lld may not support. Use a recent SDK.
* **SDK discovery from git.** `bootstrap` lists what a repository offers, but
  the sparse/blobless clone has never been run. Expect the SDK to be large and
  the checkout slow, and expect to verify the result by hand. The default
  source is a third-party repository, not Apple's: check what it publishes
  before relying on it.

## 2. Out of scope by design

Detected, reported with a `warning:` naming the tool that would be needed, and
skipped. A build will succeed, but the resulting app will be missing those
features — the build output repeats how many items were skipped.

* **Storyboards and XIBs** (`ibtool`) — no nib compilation, no
  `UILaunchStoryboardName`.
* **Asset catalogs** (`actool`) — no `Assets.car`, no app icons. `CFBundleIcons`
  is never generated, so the app shows a generic icon.
* **Core Data models** (`momc`).
* **Metal / SpriteKit** shaders (`metal`, `skstool`).
* **`.xcodeproj` / `.xcworkspace` parsing** — list sources in `darwinforge.toml`.
* **Prebuilt `.framework` and `.bundle` embedding** — not copied.
* **Real certificate signing.** `ldid -S` writes an **ad-hoc (fake)
  signature**. That is accepted by jailbroken devices and by tools like
  AltStore/Sideloadly only in specific workflows; it is **not** a valid App
  Store or normal-sideload signature. `apple-codesign` is a stub behind the
  `Signer` trait that fails loudly rather than producing an unsigned app.
* **Entitlements, App Groups, push notifications, keychain sharing, iCloud,
  HealthKit, CarPlay** — anything requiring entitlements.
* **Simulator / watchOS / tvOS / Mac Catalyst** slices. `arm64` device only.
* **Bitcode**, **dSYM generation**, **symbol stripping** beyond link defaults.
* **Universal binaries** (arm64 + arm64e). `--arch` accepts only `arm64`.
* **Incrementality.** Every build recompiles every source. There is no
  dependency tracking or caching.
* **Parallel builds.** Compilation is sequential.
* **Localization beyond resources.** `.lproj` directories are preserved and
  `.strings` files are copied, but no `genstrings`/`altool` step runs.

## 3. Deliberate implementation constraints

* **ZIP entries are STORE (uncompressed).** The built-in writer does not
  implement DEFLATE. An `.ipa` is therefore larger than one produced by Xcode.
  This keeps the archive byte-for-byte deterministic and dependency-free. Set
  `zip = true` in `darwinforge.toml` to use the system `zip` instead.
* **The TOML reader is a subset.** Tables, dotted keys, strings, integers,
  floats, booleans and arrays (inline or multi-line) are supported. Arrays of
  tables (`[[x]]`), inline tables, dates and multi-line basic strings are not,
  and produce an error with a line number.
* **Glob support is `**`, `*` and `?`.** No character classes (`[abc]`), no
  brace expansion (`{a,b}`).
* **Resources are copied, not compiled.** No nib, no asset catalog, no
  Core Data merge step.
* **Symlinks inside the bundle are dereferenced** and stored as regular files,
  so the `.ipa` survives transfer to a device.

## 4. Packaging is unproven locally

Every distributable artefact is produced by the GitHub Actions release workflow,
not by hand, and none of them has been built or installed on the development
machine:

| Artefact | Producer | Verified locally? |
| --- | --- | --- |
| `darwinforge_<version>_x86_64-unknown-linux-musl.tar.gz` | `scripts/release.sh` | **No** — needs a musl target and a POSIX host |
| `darwinforge_<version>_aarch64-unknown-linux-musl.tar.gz` | `scripts/release.sh` | **No** — also optional; skipped when the target is absent |
| `darwinforge_<version>_amd64.deb` | `packaging/deb/build-deb.sh` | **No** — `dpkg-deb` absent |
| `darwinforge-<version>-1*.rpm` | `packaging/rpm/darwinforge.spec` | **No** — `rpmbuild` absent |
| `darwinforge-x86_64-pc-windows-msvc.zip` | `scripts/release.ps1` | **No** — never produced |
| Arch package | `packaging/arch/PKGBUILD` | **No** — `makepkg` absent |

What *was* checked on the development machine: `shellcheck` 0.11.0 and `sh -n`
over `scripts/install.sh`, `scripts/release.sh`,
`packaging/deb/build-deb.sh` and `packaging/smoke/install-and-run.sh` (all clean),
`scripts/install.sh` actually run into a throwaway prefix, and
`scripts/install.ps1` actually run into a throwaway directory. The version is
read from `Cargo.toml` by every one of these, so packaging metadata cannot drift
from the binary — but a script that reads a version correctly is not a package
that installs.

The `.deb`, `.rpm` and Arch tarball install paths are exercised only by
`packaging/smoke/install-and-run.sh` inside containers in the release workflow,
which has not been observed running.

## 5. Environment variable overrides

Handy for testing, but they also mean a wrong value silently changes the
toolchain used:

| Variable | Effect |
| --- | --- |
| `DARWINFORGE_SDK` | default for `--sdk` |
| `DARWINFORGE_CLANG` | clang binary |
| `DARWINFORGE_LINKER` | linker binary |
| `DARWINFORGE_LDID` | ldid binary |
| `DARWINFORGE_SWIFTC` | swiftc binary |
| `DARWINFORGE_TEST_SDK` | SDK for the opt-in real-toolchain test |

The global `config.toml` that `bootstrap` writes is **not** in this list because
nothing reads it during a build. See §1.

The former `IPAFORGE_*` names of these variables are still read as a fallback,
with a deprecation warning, because the tool was renamed from `ipaforge` to
`darwinforge`. The old `ipaforge.toml` project file is likewise still loaded if
`darwinforge.toml` is absent.

## 6. Legal

* Apple's SDK is **not** included in this repository and darwinforge never
  redistributes it. `build` never downloads one; only `bootstrap` can fetch an
  SDK, and only from a third-party git repository you configure or accept. You
  are responsible for obtaining and licensing it, and for complying with
  Apple's licence terms.
* Apple, iPhone, iOS and `.ipa` are trademarks of Apple Inc.; this project is
  unaffiliated with Apple.
* An ad-hoc-signed `.ipa` is not an App Store submission.