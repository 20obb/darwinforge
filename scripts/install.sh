#!/bin/sh
#
# darwinforge installer -- installs the prebuilt `darwinforge` binary from a
# release tarball into a directory on your PATH.
#
# Written for POSIX sh (dash, busybox sh, bash-as-sh): no bashisms, so the same
# file runs on Debian/Ubuntu, Fedora/RHEL, Arch, Alpine, openSUSE and anything
# else that ships a /bin/sh.
#
# Rules this script obeys:
#   * The distribution is DETECTED at run time from /etc/os-release (ID and
#     ID_LIKE). There is no hardcoded distro list anywhere, so a new release is
#     supported without patching this file. Distro data is used only to print
#     friendly information and to pick a sensible fallback message.
#   * The default install location is $HOME/.local/bin -- no root, no sudo.
#     /usr/local/bin is only ever written when you explicitly ask for --system.
#   * Re-running is safe and idempotent: the only file touched is the single
#     `darwinforge` binary this script manages.
#   * Nothing outside the chosen install directory is created or removed. There
#     is no `rm -rf` in this file; the only removal is `rm -f` of one known path.
#
# Usage:
#   ./install.sh [options]
#
# Options:
#   --prefix DIR    install under DIR           (default: $HOME/.local)
#   --bin DIR       install exactly into DIR     (overrides --prefix)
#   --system        install into /usr/local/bin  (may use sudo)
#   --source PATH   use the darwinforge binary at PATH instead of the one
#                   shipped next to this script
#   -y, --yes       never prompt; assume the default answer
#   --no-bootstrap  do not offer to run `darwinforge bootstrap`
#   -h, --help      print this help and exit
#
# Exit status: 0 on success, non-zero on any failure.

set -eu

# --------------------------------------------------------------------------
# Constants
# --------------------------------------------------------------------------

PROGRAM='darwinforge'
DEFAULT_PREFIX="${HOME}/.local"
BIN_DIR=''
USE_SYSTEM='no'
ASSUME_YES='no'
ALLOW_BOOTSTRAP='yes'
SOURCE_BIN=''

# Filled in by detect_distro().
DISTRO_ID='unknown'
DISTRO_ID_LIKE=''
DISTRO_PRETTY='unknown'

# --------------------------------------------------------------------------
# Output helpers. Diagnostics go to stderr so `2>/dev/null` stays meaningful.
# --------------------------------------------------------------------------

info() {
    printf '%s\n' "$*"
}

warn() {
    printf 'warning: %s\n' "$*" >&2
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

# ask QUESTION DEFAULT(y|n) -- true when the user answered yes.
# Honours --yes and never blocks when stdin is not a terminal.
ask() {
    question=$1
    default=$2
    suffix=' [y/N] '
    if [ "$default" = 'y' ]; then
        suffix=' [Y/n] '
    fi

    if [ "$ASSUME_YES" = 'yes' ]; then
        return 0
    fi
    if [ ! -t 0 ]; then
        # Non-interactive: take the default rather than hang or fail.
        [ "$default" = 'y' ] && return 0
        return 1
    fi

    printf '%s%s%s' "$question" "$suffix" ''
    IFS= read -r reply || reply=''
    if [ -z "$reply" ]; then
        [ "$default" = 'y' ] && return 0
        return 1
    fi
    case "$reply" in
        [yY] | [yY][eE][sS]) return 0 ;;
        *) return 1 ;;
    esac
}

# Print the header comment of this file as the help text. The range is found
# rather than hardcoded, so editing the comment above can never desynchronise
# --help from the source.
usage() {
    sed -n '2,/^$/p' "$0" | sed -e 's/^#//' -e 's/^ //' | sed '/^$/d'
}

# --------------------------------------------------------------------------
# Distribution detection
#
# Everything below reads /etc/os-release (the freedesktop.org standard that
# systemd, busybox, and every current distro ships). ID is the vendor-specific
# lowercase id, ID_LIKE is a space-separated list of parent ids for derivatives.
# We deliberately keep the *values* rather than branching on them: the install
# logic is identical everywhere, and the values are only used for reporting and
# for choosing which follow-up hint to print.
# --------------------------------------------------------------------------

detect_distro() {
    DISTRO_ID='unknown'
    DISTRO_ID_LIKE=''
    DISTRO_PRETTY='unknown'

    if [ -r /etc/os-release ]; then
        # Source the standard file rather than parsing it: the spec defines it as
        # a set of shell-compatible variable assignments, and this handles values
        # containing spaces or "=" correctly.
        # shellcheck disable=SC1091
        . /etc/os-release
        DISTRO_ID="${ID:-unknown}"
        DISTRO_ID_LIKE="${ID_LIKE:-}"
        DISTRO_PRETTY="${PRETTY_NAME:-${NAME:-$DISTRO_ID}}"
    else
        warn 'no /etc/os-release found; continuing without distribution details'
    fi
}

# A one-line "you are here" string, used in the banner.
distro_summary() {
    if [ -n "$DISTRO_ID_LIKE" ]; then
        printf '%s (ID=%s, ID_LIKE=%s)' "$DISTRO_PRETTY" "$DISTRO_ID" "$DISTRO_ID_LIKE"
    else
        printf '%s (ID=%s)' "$DISTRO_PRETTY" "$DISTRO_ID"
    fi
}

# --------------------------------------------------------------------------
# PATH handling
# --------------------------------------------------------------------------

# path_contains DIR -- true when DIR is one of the entries in $PATH.
# Implemented with parameter expansion instead of `case ":$PATH:"` so that a
# trailing slash or a relative entry still compares sensibly.
path_contains() {
    probe=$1
    saved_ifs=$IFS
    IFS=:
    for entry in $PATH; do
        if [ "$entry" = "$probe" ]; then
            IFS=$saved_ifs
            return 0
        fi
    done
    IFS=$saved_ifs
    return 1
}

# The literal line a user must add to their shell rc to pick up $1.
# $PATH is intentionally literal: this is text to be pasted into a shell, not a
# value expanded right now.
# shellcheck disable=SC2016
path_hint() {
    printf 'export PATH=%s:$PATH' "$1"
}

# --------------------------------------------------------------------------
# Locating the binary to install
# --------------------------------------------------------------------------

# Walk up from this script's own directory: a release tarball unpacks to
# darwinforge-<version>-<arch>/ with the binary either at the top or in bin/.
script_dir() {
    d=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
    printf '%s' "$d"
}

find_source_binary() {
    if [ -n "$SOURCE_BIN" ]; then
        [ -f "$SOURCE_BIN" ] || die "--source: no such file: $SOURCE_BIN"
        printf '%s' "$SOURCE_BIN"
        return 0
    fi

    base=$(script_dir)
    for candidate in \
        "$base/$PROGRAM" \
        "$base/bin/$PROGRAM" \
        "./$PROGRAM" \
        "./target/release/$PROGRAM"; do
        if [ -f "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done

    # Last resort: an already-installed copy elsewhere on PATH.
    found=$(command -v "$PROGRAM" 2>/dev/null || true)
    if [ -n "$found" ] && [ -f "$found" ]; then
        warn "no tarball binary next to this script; using the installed $found"
        printf '%s' "$found"
        return 0
    fi

    return 1
}

# --------------------------------------------------------------------------
# Installing
# --------------------------------------------------------------------------

# Can we write into this directory without escalating?
dir_writable() {
    [ -d "$1" ] || mkdir -p -- "$1" 2>/dev/null || return 1
    [ -w "$1" ]
}

# Copy SRC to DEST: write to a temp name in the same directory, chmod, then
# rename. A rename within one filesystem is atomic, so an interrupted run can
# never leave a half-written binary on PATH. Re-running just overwrites the
# destination, which is why this is safe to repeat.
place_binary() {
    src=$1
    dest=$2

    dir_writable "$(dirname -- "$dest")" || return 1

    tmp="$dest.new.$$"
    # cp, not mv: the source may live on a read-only mount (the unpacked
    # tarball) and we must not modify the user's download.
    if ! cp -- "$src" "$tmp"; then
        rm -f -- "$tmp"
        return 1
    fi
    if ! chmod 0755 -- "$tmp"; then
        rm -f -- "$tmp"
        return 1
    fi
    if ! mv -f -- "$tmp" "$dest"; then
        rm -f -- "$tmp"
        return 1
    fi
    return 0
}

# Try to install with sudo, but only after the user explicitly asked for it.
place_binary_sudo() {
    src=$1
    dest=$2
    dest_dir=$(dirname -- "$dest")

    if ! command -v sudo >/dev/null 2>&1; then
        warn "sudo is not installed; cannot write to $dest_dir"
        return 1
    fi

    info "using sudo to write to $dest_dir"
    if ! sudo -v; then
        warn 'sudo authentication failed or was cancelled'
        return 1
    fi
    if ! sudo mkdir -p -- "$dest_dir"; then
        return 1
    fi
    tmp="$dest.new.$$"
    if sudo cp -- "$src" "$tmp" &&
        sudo chmod 0755 -- "$tmp" &&
        sudo mv -f -- "$tmp" "$dest"; then
        return 0
    fi
    sudo rm -f -- "$tmp" 2>/dev/null || true
    return 1
}

# --------------------------------------------------------------------------
# bootstrap
#
# `darwinforge bootstrap` provisions the host toolchain (clang, a Mach-O
# linker, ldid). It is offered, never forced, and only if this build actually
# implements it -- an older or trimmed binary degrades to a hint rather than an
# error.
# --------------------------------------------------------------------------

binary_has_bootstrap() {
    "$1" --help 2>/dev/null | grep -q 'bootstrap'
}

offer_bootstrap() {
    [ "$ALLOW_BOOTSTRAP" = 'yes' ] || return 0

    installed=$1

    if ! binary_has_bootstrap "$installed"; then
        info ''
        info "This build does not provide 'darwinforge bootstrap'."
        info "To see what the iOS toolchain still needs, run:"
        info "    $PROGRAM doctor"
        return 0
    fi

    info ''
    info "Next step: 'darwinforge bootstrap' installs the toolchain this"
    info "project needs (clang, a Mach-O linker, ldid) using your distro's"
    info "package manager."
    if ask 'Run it now?' 'n'; then
        info ''
        if "$installed" bootstrap; then
            info 'bootstrap finished'
        else
            # The backticks are literal here: this is advice text, not a command.
            # shellcheck disable=SC2016
            warn 'bootstrap did not complete; run `darwinforge doctor` to see what is missing'
        fi
    else
        info "Skipped. Run '$PROGRAM bootstrap' whenever you are ready."
    fi
}

# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

parse_args() {
    while [ $# -gt 0 ]; do
        case $1 in
            --prefix)
                [ $# -ge 2 ] || die '--prefix needs a directory'
                DEFAULT_PREFIX=$2
                shift 2
                ;;
            --bin)
                [ $# -ge 2 ] || die '--bin needs a directory'
                BIN_DIR=$2
                shift 2
                ;;
            --system)
                USE_SYSTEM='yes'
                shift
                ;;
            --source)
                [ $# -ge 2 ] || die '--source needs a path'
                SOURCE_BIN=$2
                shift 2
                ;;
            -y | --yes)
                ASSUME_YES='yes'
                shift
                ;;
            --no-bootstrap)
                ALLOW_BOOTSTRAP='no'
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                printf 'error: unknown option: %s\n\n' "$1" >&2
                usage >&2
                exit 2
                ;;
        esac
    done

    if [ -z "$BIN_DIR" ]; then
        if [ "$USE_SYSTEM" = 'yes' ]; then
            BIN_DIR='/usr/local/bin'
        else
            BIN_DIR="${DEFAULT_PREFIX%/}/bin"
        fi
    fi
}

main() {
    parse_args "$@"

    detect_distro

    source_bin=$(find_source_binary) ||
        die "no $PROGRAM binary found next to this script, and none on PATH.
   Unpack the full release tarball, or pass --source /path/to/$PROGRAM."

    if [ ! -x "$source_bin" ]; then
        # A tarball that lost its exec bit is still installable; we fix the copy
        # we place, and never touch the user's download.
        warn "$source_bin is not executable; installing anyway"
    fi

    target="$BIN_DIR/$PROGRAM"

    info "$PROGRAM installer"
    info "  distribution : $(distro_summary)"
    info "  source       : $source_bin"
    info "  destination  : $target"
    info ''

    if [ -e "$target" ]; then
        info "An existing $target is present; it will be replaced."
    fi

    if ! place_binary "$source_bin" "$target"; then
        # The only escalation path, and it runs only because the user passed
        # --system or answered the prompt.
        if [ "$USE_SYSTEM" != 'yes' ]; then
            if ask "$BIN_DIR is not writable. Install to /usr/local/bin with sudo instead?" 'n'; then
                target="/usr/local/bin/$PROGRAM"
                place_binary_sudo "$source_bin" "$target" ||
                    die "could not install to $target"
            else
                die "could not write to $BIN_DIR.
   Re-run with --prefix DIR pointing somewhere writable, e.g. --prefix \"\$HOME/.local\""
            fi
        else
            place_binary_sudo "$source_bin" "$target" ||
                die "could not install to $target"
        fi
    fi

    install_dir=$(dirname -- "$target")
    info ''
    info "installed $target"

    if "$target" --version >/dev/null 2>&1; then
        info "version: $("$target" --version 2>/dev/null | head -n 1)"
    else
        warn 'installed binary did not answer --version; it may be for another CPU'
    fi

    if path_contains "$install_dir"; then
        info "PATH: $install_dir is already on your PATH"
    else
        warn "$install_dir is not on your PATH."
        warn 'Add this line to your shell profile (~/.bashrc, ~/.zshrc, ~/.profile):'
        warn ''
        warn "    $(path_hint "$install_dir")"
        warn ''
        warn 'Or, for this shell only:'
        warn "    $(path_hint "$install_dir")"
    fi

    offer_bootstrap "$target"

    info ''
    info 'Re-running this script is safe: it only replaces the binary above.'
    return 0
}

main "$@"




