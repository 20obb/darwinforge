//! Stage 5: code signing.
//!
//! The PoC uses `ldid -S`, which adds an ad-hoc (fake) signature. Real
//! certificate signing is out of scope, but the signer sits behind a trait so
//! `apple-codesign` can be dropped in without touching the pipeline.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::exec;
use crate::reporter::Reporter;

/// Something that can put a signature on an app bundle's Mach-O.
pub trait Signer {
    /// Human-readable name, shown in build output.
    fn name(&self) -> &'static str;

    /// Sign the executable inside `app_bundle`.
    fn sign(&self, app_bundle: &Path, reporter: &Reporter) -> Result<()>;
}

/// Ad-hoc signer backed by `ldid`.
pub struct LdidSigner {
    ldid: PathBuf,
    /// Optional entitlements plist, applied as `ldid -S<file>`.
    entitlements: Option<PathBuf>,
}

impl LdidSigner {
    pub fn new(ldid: PathBuf) -> LdidSigner {
        LdidSigner { ldid, entitlements: None }
    }

    pub fn with_entitlements(mut self, entitlements: PathBuf) -> LdidSigner {
        self.entitlements = Some(entitlements);
        self
    }

    /// The exact command line, exposed for tests.
    pub fn arguments(&self, executable: &Path) -> Vec<String> {
        let mut args = Vec::new();
        match &self.entitlements {
            // ldid spells "sign with entitlements" as -S<file>, no space.
            Some(path) => args.push(format!("-S{}", path.to_string_lossy())),
            None => args.push("-S".to_string()),
        }
        args.push(executable.to_string_lossy().to_string());
        args
    }
}

impl Signer for LdidSigner {
    fn name(&self) -> &'static str {
        "ldid (ad-hoc / fake signature)"
    }

    fn sign(&self, app_bundle: &Path, reporter: &Reporter) -> Result<()> {
        // `ldid` signs the Mach-O itself, not the .app directory.
        let executable_name = app_bundle
            .file_name()
            .map(|name| name.to_string_lossy().trim_end_matches(".app").to_string())
            .unwrap_or_else(|| "app".to_string());
        let executable = app_bundle.join(executable_name);
        exec::run(
            &self.ldid.to_string_lossy(),
            &self.arguments(&executable),
            None,
            reporter,
        )
    }
}

/// Placeholder for a real, certificate-backed signer. Selecting it fails loudly
/// rather than producing an unsigned app.
pub struct AppleCodesignSigner {
    pub identity: String,
}

impl Signer for AppleCodesignSigner {
    fn name(&self) -> &'static str {
        "apple-codesign"
    }

    fn sign(&self, _app_bundle: &Path, _reporter: &Reporter) -> Result<()> {
        Err(crate::error::Error::Unsupported {
            message: "apple-codesign signing is a planned extension point, not \
                      implemented in this PoC.\n  \
                      darwinforge signs with `ldid -S`, which produces an ad-hoc \
                      (fake) signature good enough for jailbroken-device sideloading \
                      and for validating the bundle layout, but NOT for App Store \
                      or normal sideloading."
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ldid_signs_the_binary_with_dash_capital_s() {
        let signer = LdidSigner::new(PathBuf::from("/usr/bin/ldid"));
        let args = signer.arguments(Path::new("/build/Hello.app/Hello"));
        assert_eq!(args, vec!["-S".to_string(), "/build/Hello.app/Hello".to_string()]);
    }

    #[test]
    fn entitlements_are_passed_inline() {
        let signer = LdidSigner::new(PathBuf::from("/usr/bin/ldid"))
            .with_entitlements(PathBuf::from("/build/ent.plist"));
        let args = signer.arguments(Path::new("/build/Hello.app/Hello"));
        assert_eq!(
            args,
            vec!["-S/build/ent.plist".to_string(), "/build/Hello.app/Hello".to_string()]
        );
    }

    #[test]
    fn signer_reports_its_name() {
        let signer = LdidSigner::new(PathBuf::from("/usr/bin/ldid"));
        assert!(signer.name().contains("ldid"));
    }

    #[test]
    fn apple_codesign_stub_fails_loudly() {
        let signer = AppleCodesignSigner { identity: "iPhone Developer".to_string() };
        let reporter = Reporter::new(false);
        let error = signer.sign(Path::new("/build/Hello.app"), &reporter).expect_err("must fail");
        assert!(matches!(error, crate::error::Error::Unsupported { .. }));
        assert!(error.to_string().contains("not implemented"));
    }
}