#!/bin/sh
#
# release.sh -- build the Linux release artefacts into ./dist
#
# Deliberately make-free: the whole release is three cargo invocations plus tar,
# dpkg-deb and sha256sum, so it works on a bare container with no build-system
# package installed.
#
# What it produces in ./dist:
#   darwinforge_<version>_x86_64-unknown-linux-musl.tar.gz
#   darwinforge_<version>_aarch64-unknown-linux-musl.tar.gz   (if the target is
#                                                             installed)
#   darwinforge_<version>_amd64.deb
#   SHA256SUMS
#
# The version is read from Cargo.toml. It is never written into this script.
#
# Usage:
#   scripts/release.sh [options]
#
# Options:
#   --dry-run        print the plan and exit without building anything
#   --out-dir DIR    write artefacts to DIR            (default: dist)
#   --skip-tests     do NOT run cargo test (not recommended; CI runs them)
#   --only TARGET    build only 'x86_64', 'aarch64', 'deb', 'checksums'
#   --no-deb         skip the Debian package (needs dpkg-deb)
#   -h, --help       this text
#
# Requirements: cargo with the musl targets, and (for the .deb) dpkg-deb.
#
#   rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

set -eu

PACKAGE='darwinforge'
REPO_ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
OUT_DIR="$REPO_ROOT/dist"
DRY_RUN='no'
SKIP_TESTS='no'
ONLY=''
BUILD_DEB='yes'

# Reproducible output when the caller pins it, e.g.
#   SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) scripts/release.sh
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
export SOURCE_DATE_EPOCH

# Shellcheck directives are only needed in this file, not in the generated
# scripts, so `disable` is declared locally around the few intentional cases.
# shellcheck disable=SC2034

# Diagnostics go to stderr so that stdout stays usable for piping. This is not
# cosmetic here: stage_target() is called as `pkgdir=$(stage_target ...)`, so
# anything it prints on stdout would end up inside the variable.
info() { printf '==> %s\n' "$*" >&2; }
step() { printf '  - %s\n' "$*" >&2; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --------------------------------------------------------------------------
# Cargo.toml is the single source of truth for the version.
# --------------------------------------------------------------------------
crate_version() {
    manifest="$REPO_ROOT/Cargo.toml"
    [ -f "$manifest" ] || die "no Cargo.toml at $manifest"
    found=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$manifest" | head -n 1)
    [ -n "$found" ] || die "could not read \`version = \"...\"\` from $manifest"
    printf '%s' "$found"
}

# Is this rustup target actually installed? Skipping gracefully (rather than
# failing the whole release) is what lets the aarch64 tarball be optional.
target_installed() {
    rustup target list --installed 2>/dev/null | grep -qx "$1"
}

# --------------------------------------------------------------------------
# Artefact names
# --------------------------------------------------------------------------

tarball_name() {
    target=$1
    printf '%s_%s_%s.tar.gz' "$PACKAGE" "$VERSION" "$target"
}

# --------------------------------------------------------------------------
# Building one musl target
# --------------------------------------------------------------------------

# Build the staging tree for a target: a single top-level directory so the
# tarball unpacks into one folder, containing the binary, the docs, the
# licence and the installer.
stage_target() {
    target=$1
    pkgdir="$OUT_DIR/stage/${PACKAGE}_${VERSION}_${target}"

    rm -rf "$pkgdir"
    mkdir -p "$pkgdir"

    info "building $target"
    step "cargo build --release --target $target"
    ( cd "$REPO_ROOT" && cargo build --release --target "$target" )

    binary="$REPO_ROOT/target/$target/release/$PACKAGE"
    [ -f "$binary" ] || die "cargo did not produce $binary"

    step "staging $(basename "$pkgdir")"
    install -m 0755 "$binary" "$pkgdir/$PACKAGE"
    for doc in README.md LIMITATIONS.md; do
        [ -f "$REPO_ROOT/$doc" ] && install -m 0644 "$REPO_ROOT/$doc" "$pkgdir/$doc"
    done
    install -m 0644 "$REPO_ROOT/LICENSE" "$pkgdir/LICENSE"
    install -m 0755 "$REPO_ROOT/scripts/install.sh" "$pkgdir/install.sh"

    # Fixed timestamps and ownership so the tarball is reproducible.
    find "$pkgdir" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} + 2>/dev/null || true

    printf '%s' "$pkgdir"
}

# Pack a staged directory into dist/<name>, deterministically.
pack_target() {
    target=$1
    pkgdir=$2
    out="$OUT_DIR/$(tarball_name "$target")"

    info "packing $(basename "$out")"
    rm -f "$out"
    # --sort=name gives a stable member order; --mtime/--owner/--group pin the
    # metadata; --numeric-owner avoids name lookups that could differ per host.
    if tar --sort=name \
        --mtime="@$SOURCE_DATE_EPOCH" \
        --owner=0 --group=0 --numeric-owner \
        --format=gnu \
        -C "$(dirname -- "$pkgdir")" -czf "$out" "$(basename -- "$pkgdir")"; then
        step "ok ($(du -h "$out" | awk '{print $1}'))"
    else
        # Older tar (busybox, some BSD userlands) lacks --sort; fall back to a
        # plain archive so the release still completes.
        warn "deterministic tar failed; retrying without --sort"
        tar -C "$(dirname -- "$pkgdir")" -czf "$out" "$(basename -- "$pkgdir")"
    fi
}

# --------------------------------------------------------------------------
# Tests -- run before anything is packaged.
# --------------------------------------------------------------------------

run_tests() {
    info 'running the test suite (a failure here aborts the release)'
    step 'cargo test'
    if ! ( cd "$REPO_ROOT" && cargo test ); then
        die 'cargo test failed; refusing to package a release that does not pass its own tests'
    fi
    step 'ok'
}

# --------------------------------------------------------------------------
# SHA256SUMS
# --------------------------------------------------------------------------

# Written over every regular file in dist/, sorted, so `sha256sum -c` works
# from inside dist/. Uses sha256sum, falling back to shasum -a 256.
write_checksums() {
    info 'writing SHA256SUMS'
    out="$OUT_DIR/SHA256SUMS"
    rm -f "$out"

    # Only artefacts, not the staging directory or the sums file itself.
    (
        cd "$OUT_DIR" || exit 1
        find . -maxdepth 1 -type f ! -name 'SHA256SUMS' -printf '%P\n' |
            LC_ALL=C sort |
            while IFS= read -r name; do
                [ -n "$name" ] || continue
                if command -v sha256sum >/dev/null 2>&1; then
                    sha256sum "$name"
                elif command -v shasum >/dev/null 2>&1; then
                    shasum -a 256 "$name"
                else
                    die 'neither sha256sum nor shasum found; cannot write SHA256SUMS'
                fi
            done
    ) >"$out"

    step "$(wc -l <"$out" | tr -d ' ') file(s) hashed"
}

# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

# Print the header comment of this file as the help text. The range is found
# rather than hardcoded, so editing the comment above can never desynchronise
# --help from the source.
usage() {
    sed -n '2,/^$/p' "$0" | sed -e 's/^#//' -e 's/^ //' | sed '/^$/d'
}

parse_args() {
    while [ $# -gt 0 ]; do
        case $1 in
            --dry-run)
                DRY_RUN='yes'
                shift
                ;;
            --out-dir)
                [ $# -ge 2 ] || die '--out-dir needs a directory'
                OUT_DIR=$2
                shift 2
                ;;
            --skip-tests)
                SKIP_TESTS='yes'
                shift
                ;;
            --only)
                [ $# -ge 2 ] || die '--only needs a target name'
                ONLY=$2
                shift 2
                ;;
            --no-deb)
                BUILD_DEB='no'
                shift
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

    # A relative --out-dir is relative to the caller, not to the repo.
    case $OUT_DIR in
        /*) ;;
        *) OUT_DIR="$PWD/$OUT_DIR" ;;
    esac
}

wants() {
    [ -z "$ONLY" ] && return 0
    [ "$ONLY" = "$1" ]
}

main() {
    parse_args "$@"

    VERSION=$(crate_version)

    info "darwinforge $VERSION"
    info "  repo : $REPO_ROOT"
    info "  out  : $OUT_DIR"
    info "  epoch: $SOURCE_DATE_EPOCH"

    if [ "$DRY_RUN" = 'yes' ]; then
        info ''
        info 'dry run: no artefacts will be built'
        info ''
        step 'would run: cargo test'
        if target_installed 'x86_64-unknown-linux-musl'; then
            step "would build: $(tarball_name 'x86_64-unknown-linux-musl')"
        else
            warn 'target x86_64-unknown-linux-musl is not installed; run: rustup target add x86_64-unknown-linux-musl'
        fi
        if target_installed 'aarch64-unknown-linux-musl'; then
            step "would build: $(tarball_name 'aarch64-unknown-linux-musl')"
        else
            step 'would skip: aarch64 (rustup target not installed)'
        fi
        if [ "$BUILD_DEB" = 'yes' ]; then
            if command -v dpkg-deb >/dev/null 2>&1; then
                step "would build: ${PACKAGE}_${VERSION}_amd64.deb"
            else
                warn 'dpkg-deb not found; the .deb would be skipped'
            fi
        fi
        step 'would write: SHA256SUMS'
        exit 0
    fi

    command -v cargo >/dev/null 2>&1 || die 'cargo not found on PATH'

    mkdir -p "$OUT_DIR"

    # 1. Tests first: never package something that does not pass its own tests.
    if [ "$SKIP_TESTS" = 'yes' ]; then
        warn 'skipping cargo test because --skip-tests was given'
    elif [ -z "$ONLY" ]; then
        run_tests
    else
        warn "skipping cargo test (--only $ONLY)"
    fi

    # 2. The two musl tarballs.
    for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
        short=${target%%-unknown-*}
        if ! wants "$short" && ! [ -z "$ONLY" ]; then
            continue
        fi

        if ! target_installed "$target"; then
            warn "skipping $target: rustup target not installed"
            step "install it with: rustup target add $target"
            continue
        fi

        pkgdir=$(stage_target "$target")
        pack_target "$target" "$pkgdir"
    done

    # 3. The Debian package.
    if [ "$BUILD_DEB" = 'yes' ] && { [ -z "$ONLY" ] || [ "$ONLY" = 'deb' ]; }; then
        if command -v dpkg-deb >/dev/null 2>&1; then
            if [ -f "$REPO_ROOT/target/x86_64-unknown-linux-musl/release/$PACKAGE" ]; then
                info 'building the Debian package'
                sh "$REPO_ROOT/packaging/deb/build-deb.sh" \
                    --binary "$REPO_ROOT/target/x86_64-unknown-linux-musl/release/$PACKAGE" \
                    --out-dir "$OUT_DIR" \
                    --arch amd64
            else
                warn 'skipping the .deb: no x86_64 musl binary was built'
            fi
        else
            warn 'skipping the .deb: dpkg-deb not found'
        fi
    fi

    # The staging tree is scratch; remove it so dist/ holds only artefacts.
    rm -rf "$OUT_DIR/stage"

    # 4. Checksums over whatever we produced.
    if [ -z "$ONLY" ] || [ "$ONLY" = 'checksums' ]; then
        write_checksums
    fi

    info ''
    info 'artefacts:'
    for f in "$OUT_DIR"/*; do
        [ -f "$f" ] || continue
        step "$(basename -- "$f")  ($(du -h "$f" | awk '{print $1}'))"
    done

    return 0
}

main "$@"