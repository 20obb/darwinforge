#!/bin/sh
#
# build-deb.sh -- produce a Debian/Ubuntu package for darwinforge.
#
# The version is read from Cargo.toml at build time; it is never written into
# this script. That keeps the package metadata and the binary in lockstep: bump
# `version` in Cargo.toml, run this, and the .deb is correct.
#
# Usage:
#   packaging/deb/build-deb.sh [--binary PATH] [--out-dir DIR] [--arch ARCH]
#
# Options:
#   --binary PATH   the darwinforge binary to package
#                   (default: target/x86_64-unknown-linux-musl/release/darwinforge,
#                    then target/release/darwinforge)
#   --out-dir DIR   where to write the .deb        (default: dist)
#   --arch ARCH     Debian architecture             (default: amd64)
#   -h, --help      this text
#
# Requires: dpkg-deb (dpkg), coreutils. Run on a Debian-family machine or in a
# container; `dpkg-deb --build` only needs the ar/tar plumbing, so a Fedora
# host with `dpkg` installed works too.

set -eu

PACKAGE='darwinforge'
# Overridable so a fork does not have to edit this file: build-deb.sh reads it
# from the environment when set. Debian requires a real, deliverable address.
MAINTAINER="${MAINTAINER:-20obb <20obb@users.noreply.github.com>}"
HOMEPAGE='https://github.com/20obb/darwinforge'
SECTION='devel'
PRIORITY='optional'
# The short description must stay on ONE line: deb-control treats a leading
# space on the next line as the start of the extended description, which is
# where the indented lines below go.
SHORT_DESC='Build an installable iOS .ipa from C/ObjC/C++/Swift sources on Linux'

# emit_long_desc -- print the extended description, one sentence-ish per line,
# each prefixed with a single space as deb-control(5) requires.
emit_long_desc() {
    printf ' %s\n' 'darwinforge turns a small iOS project written in C, Objective-C, C++'
    printf ' %s\n' 'or Swift into an installable .ipa on Linux, without a Mac and'
    printf ' %s\n' 'without a remote build host.'
    printf ' %s\n' ''
    printf ' %s\n' 'It orchestrates tools that already exist (clang, ld64.lld, ldid)'
    printf ' %s\n' 'instead of reimplementing them, and it never downloads the Apple'
    printf ' %s\n' 'SDK: you supply an extracted iPhoneOS SDK yourself.'
    printf ' %s\n' ''
    printf ' %s\n' 'Apple, iPhone, iOS and .ipa are trademarks of Apple Inc. This'
    printf ' %s\n' 'project is unaffiliated with Apple.'
}

REPO_ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
LICENSE_FILE="$REPO_ROOT/LICENSE"

BINARY=''
OUT_DIR="$REPO_ROOT/dist"
ARCH='amd64'

# Reproducible builds: honour SOURCE_DATE_EPOCH when the caller sets it, and
# otherwise use the mtime of the newest tracked input so two runs of the same
# tree agree. dpkg-deb and ar record mtimes, so without this the .deb differs
# byte-for-byte between builds.
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"

info() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Print the header comment of this file as the help text. The range is found
# rather than hardcoded, so editing the comment above can never desynchronise
# --help from the source.
usage() {
    sed -n '2,/^$/p' "$0" | sed -e 's/^#//' -e 's/^ //' | sed '/^$/d'
}

# The single source of truth for the version: Cargo.toml.
crate_version() {
    manifest="$REPO_ROOT/Cargo.toml"
    [ -f "$manifest" ] || die "no Cargo.toml at $manifest"

    # Anchored on `version` at the start of a line so `rust-version` (which also
    # appears in this manifest) is not matched.
    found=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest" | head -n 1)
    [ -n "$found" ] || die "could not read \`version = \"...\"\` from $manifest"

    # Debian versions accept [A-Za-z0-9.+~-]; a semver pre-release like
    # 0.1.0-rc.1 is fine, but a stray '+' from build metadata is legal too.
    case $found in
        *[!A-Za-z0-9.+~:-]*)
            die "version '$found' from Cargo.toml is not a usable Debian version"
            ;;
    esac
    printf '%s' "$found"
}

parse_args() {
    while [ $# -gt 0 ]; do
        case $1 in
            --binary)
                [ $# -ge 2 ] || die '--binary needs a path'
                BINARY=$2
                shift 2
                ;;
            --out-dir)
                [ $# -ge 2 ] || die '--out-dir needs a directory'
                OUT_DIR=$2
                shift 2
                ;;
            --arch)
                [ $# -ge 2 ] || die '--arch needs an architecture'
                ARCH=$2
                shift 2
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                die "unknown option: $1 (try --help)"
                ;;
        esac
    done
}

# Prefer a musl build: a statically linked binary is what makes this package
# usable on any Debian-family distro regardless of its glibc version. Fall back
# to whatever the host build produced, with a warning about portability.
find_binary() {
    if [ -n "$BINARY" ]; then
        [ -f "$BINARY" ] || die "--binary: no such file: $BINARY"
        printf '%s' "$BINARY"
        return 0
    fi

    for candidate in \
        "$REPO_ROOT/target/x86_64-unknown-linux-musl/release/$PACKAGE" \
        "$REPO_ROOT/target/release/$PACKAGE"; do
        if [ -f "$candidate" ]; then
            [ "$candidate" = "$REPO_ROOT/target/release/$PACKAGE" ] &&
                warn 'using a glibc-linked build; the .deb will only work on a compatible glibc'
            printf '%s' "$candidate"
            return 0
        fi
    done

    return 1
}

# Normalise every file we are about to pack: fixed owner, fixed mtime, fixed
# permissions. This is what makes the .deb byte-for-byte reproducible.
normalise_tree() {
    root=$1

    # Ownership: dpkg-deb --root-owner-group makes this a no-op for the tar
    # member, but normalising here also keeps a locally inspected tree clean.
    # shellcheck disable=SC2016
    find "$root" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} + 2>/dev/null ||
        find "$root" -exec touch -d "@$SOURCE_DATE_EPOCH" {} +

    chmod 0755 "$root"
    if [ -d "$root/DEBIAN" ]; then
        chmod 0755 "$root/DEBIAN"
        chmod 0644 "$root/DEBIAN"/*
    fi
    if [ -d "$root/usr/bin" ]; then
        chmod 0755 "$root/usr/bin"
        chmod 0755 "$root/usr/bin/$PACKAGE"
    fi
}

main() {
    parse_args "$@"

    command -v dpkg-deb >/dev/null 2>&1 ||
        die 'dpkg-deb not found. Install dpkg (Debian/Ubuntu) or run this in a container.'

    version=$(crate_version)
    binary=$(find_binary) ||
        die "no $PACKAGE binary found. Build one first, e.g.
   cargo build --release --target x86_64-unknown-linux-musl
   or pass --binary /path/to/$PACKAGE"

    stage="$REPO_ROOT/target/deb/$PACKAGE"
    # `rm -rf` is confined to target/deb inside this repository -- never a
    # user-supplied path, and never anything outside the build directory.
    rm -rf "$stage"
    mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/doc/$PACKAGE"

    info "packaging $PACKAGE $version ($ARCH)"
    info "  binary    : $binary"
    info "  staging   : $stage"

    install -m 0755 "$binary" "$stage/usr/bin/$PACKAGE"

    # Documentation and licence. Debian expects the licence at
    # /usr/share/doc/<pkg>/copyright, so MIT text goes there verbatim.
    for doc in README.md LIMITATIONS.md; do
        if [ -f "$REPO_ROOT/$doc" ]; then
            install -m 0644 "$REPO_ROOT/$doc" "$stage/usr/share/doc/$PACKAGE/$doc"
        fi
    done
    if [ -f "$LICENSE_FILE" ]; then
        install -m 0644 "$LICENSE_FILE" "$stage/usr/share/doc/$PACKAGE/copyright"
    fi

    installed_size=$(du -sk "$stage" | awk '{print $1}')
    if [ -z "$installed_size" ] || [ "$installed_size" -le 0 ] 2>/dev/null; then
        installed_size=1
    fi

    cat >"$stage/DEBIAN/control" <<EOF
Package: $PACKAGE
Version: $version
Section: $SECTION
Priority: $PRIORITY
Architecture: $ARCH
Depends: git
Recommends: clang, lld, zip
Suggests: swiftlang
Maintainer: $MAINTAINER
Homepage: $HOMEPAGE
Installed-Size: $installed_size
Description: $SHORT_DESC
EOF
    emit_long_desc >>"$stage/DEBIAN/control"

    # Machine-readable copy of the licence, as /usr/share/doc/<pkg>/copyright
    # is expected to be.
    if [ -f "$LICENSE_FILE" ]; then
        install -m 0644 "$LICENSE_FILE" "$stage/usr/share/doc/$PACKAGE/copyright"
    fi

    normalise_tree "$stage"

    mkdir -p "$OUT_DIR"
    out="$OUT_DIR/${PACKAGE}_${version}_${ARCH}.deb"

    # --root-owner-group: files in the package are owned by root/root even when
    # built as an ordinary user.
    rm -f "$out"
    dpkg-deb --root-owner-group --build "$stage" "$out" >/dev/null

    info "  output    : $out"
    info "  size      : $(du -h "$out" | awk '{print $1}')"

    info ''
    info 'contents:'
    dpkg-deb --contents "$out" | sed 's/^/  /'

    return 0
}

main "$@"

