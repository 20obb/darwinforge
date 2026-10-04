# darwinforge RPM spec
#
# HOW THE VERSION GETS IN
# -----------------------
# This file deliberately contains NO hardcoded version. The version is read
# from Cargo.toml by the caller and injected at build time:
#
#     version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
#     rpmbuild -ba darwinforge.spec --define "version $version"
#
# scripts/release.sh does exactly this. If you build by hand without passing
# --define, the default below (0.0.0-dev) is used so the spec still parses --
# which makes "I forgot the --define" obvious in the output filename instead of
# producing a mysteriously broken build.
#
# The %{?version} conditional form means an already-set value always wins, so
# CI never has to edit this file and a local build never fights CI.

%global debug_package %{nil}

%{!?version:%global version 0.0.0-dev}
%global commit 0

Name:           darwinforge
Version:        %{version}
Release:        1%{?dist}
Summary:        Build an installable iOS .ipa on Linux, without a Mac

License:        MIT
URL:            https://github.com/darwinforge/darwinforge
Source0:        https://github.com/darwinforge/darwinforge/archive/v%{version}.tar.gz

# git is required because projects are usually cloned; clang/lld/zip are what
# darwinforge itself shells out to and are recommended, not required, so a
# stripped-down system can still install the binary and use `doctor` to see
# exactly what is missing.
Requires:       git
Recommends:     clang
Recommends:     lld
Recommends:     zip

BuildRequires:  cargo
BuildRequires:  rust

# The released binary is statically linked against musl, so it has no shared
# library dependencies at all. Letting rpm's automatic dependency generator
# scan it would therefore only ever find noise (or, on some platforms, a
# misleading libc6 requirement for a binary that does not use glibc). The real
# runtime requirements are declared explicitly above.
AutoReqProv:    no

%description
darwinforge turns a small iOS project written in C, Objective-C, C++ or Swift
into an installable .ipa on Linux, without a Mac and without a remote build
host.

It orchestrates tools that already exist -- clang, ld64.lld and ldid -- instead
of reimplementing them, and it never downloads the Apple SDK: you supply an
extracted iPhoneOS SDK yourself.

Apple, iPhone, iOS and .ipa are trademarks of Apple Inc. This package is
unaffiliated with Apple.

%prep
%autosetup -p1

# Reproducibility: keep the crate from phoning home. The PoC has zero
# dependencies precisely so this never fails.
%build
CARGO_NET_OFFLINE=true cargo build --release --offline --locked

%install
# shellcheck disable=SC2086
install -D -m 0755 target/release/darwinforge %{buildroot}%{_bindir}/darwinforge
install -D -m 0644 LICENSE %{buildroot}%{_licensedir}/LICENSE
install -D -m 0644 README.md %{buildroot}%{_docdir}/README.md
install -D -m 0644 LIMITATIONS.md %{buildroot}%{_docdir}/LIMITATIONS.md

%check
# The test suite is hermetic: it uses a stub toolchain and a synthetic SDK
# layout, so it runs without clang, ldid or an Apple SDK present.
CARGO_NET_OFFLINE=true cargo test --offline --locked

%files
%license %{_licensedir}/LICENSE
%doc %{_docdir}/README.md
%doc %{_docdir}/LIMITATIONS.md
%{_bindir}/darwinforge

%changelog
* Thu Jan 01 2026 DarwinForge contributors <darwinforge@example.invalid> - 0.1.0-1
- Initial packaging. Version injected from Cargo.toml at build time.