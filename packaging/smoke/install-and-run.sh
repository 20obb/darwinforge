#!/bin/sh
#
# install-and-run.sh -- install a darwinforge package in a container and prove
# the binary runs.
#
# This is the single source of truth for the package smoke test. The release
# workflow mounts the repository into a debian/fedora/archlinux container and
# runs this script inside it, so all three distros are checked by identical
# logic instead of three copies of an inline `docker run bash -c '...'` that can
# drift apart.
#
# Inputs (environment):
#   PKG    path to the artefact to install (.deb, .rpm, or .tar.gz)
#   KIND   deb | rpm | arch
#
# Assertions:
#   * the package installs with the target distro's own package manager
#   * `darwinforge --version` succeeds        <- the real assertion
#   * `darwinforge bootstrap --dry-run` runs  <- guarded; absent in this build
#
# `doctor` is printed for information but never gates the job: it deliberately
# exits non-zero when clang/ldid/an SDK are missing, which is the expected state
# of a container that only installed our package.

set -eu

PKG="${PKG:?PKG must be set}"
KIND="${KIND:?KIND must be set}"

say() { printf '\n== %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

install_deb() {
    say 'apt-get install'
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    # --no-install-recommends keeps clang/lld/zip out on purpose: they are only
    # Recommends, so a broken Recommends must never block the install.
    apt-get install -y --no-install-recommends "$PKG"
}

install_rpm() {
    say 'dnf install'
    dnf install -y "$PKG"
}

install_arch() {
    say 'install the static musl tarball'
    # No .pkg.tar.zst is published; the tarball carries the same binary, so the
    # Arch smoke test still exercises the real artefact users download.
    tmp=$(mktemp -d)
    tar xzf "$PKG" -C "$tmp"
    dir=$(find "$tmp" -maxdepth 1 -type d -name 'darwinforge_*' -print -quit)
    [ -n "$dir" ] || die 'no darwinforge_* directory in the tarball'
    install -Dm755 "$dir/darwinforge" /usr/bin/darwinforge
    rm -rf "$tmp"
}

case $KIND in
    deb) install_deb ;;
    rpm) install_rpm ;;
    arch) install_arch ;;
    *) die "unknown KIND: $KIND" ;;
esac

say 'which darwinforge'
command -v darwinforge || die 'darwinforge is not on PATH after install'

say 'darwinforge --version'
# The assertion that matters: the package installed and the binary runs here.
darwinforge --version || die 'darwinforge --version failed'

say 'darwinforge bootstrap --dry-run'
if darwinforge --help 2>&1 | grep -q bootstrap; then
    darwinforge bootstrap --dry-run || die 'bootstrap --dry-run failed'
else
    # The backticks are literal text, not command substitution.
    # shellcheck disable=SC2016
    echo 'SKIP: this build has no `bootstrap` subcommand yet'
fi

say 'darwinforge doctor (informational)'
# Expected to report missing tools and exit non-zero on a bare container.
if doctor_output=$(darwinforge doctor 2>&1); then
    printf '%s\n' "$doctor_output"
else
    printf '%s\n' "$doctor_output"
    echo '(non-zero exit is expected: the container has no clang/ldid/SDK)'
fi

say 'SMOKE TEST PASSED'