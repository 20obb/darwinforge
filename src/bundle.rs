//! Stage 3 + 4: generate `Info.plist` and assemble `Payload/<Name>.app/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::plist::{self, Plist};

/// Build the `Info.plist` for an app.
pub fn info_plist(config: &Config, executable_name: &str, arch: &str) -> Result<Plist> {
    let app = &config.app;
    let mut entries: BTreeMap<String, Plist> = BTreeMap::new();

    // Identity.
    entries.insert("CFBundleIdentifier".to_string(), Plist::String(app.bundle_id.clone()));
    entries.insert(
        "CFBundleExecutable".to_string(),
        Plist::String(executable_name.to_string()),
    );
    entries.insert(
        "CFBundleDisplayName".to_string(),
        Plist::String(app.display_name.clone().unwrap_or_else(|| app.name.clone())),
    );
    entries.insert("CFBundleName".to_string(), Plist::String(app.name.clone()));
    entries.insert("CFBundlePackageType".to_string(), Plist::String("APPL".to_string()));
    entries.insert(
        "CFBundleShortVersionString".to_string(),
        Plist::String(app.version.clone()),
    );
    entries.insert("CFBundleVersion".to_string(), Plist::String(app.build_number.clone()));
    entries.insert(
        "CFBundleInfoDictionaryVersion".to_string(),
        Plist::String("6.0".to_string()),
    );
    entries.insert(
        "CFBundleSupportedPlatforms".to_string(),
        Plist::Array(vec![Plist::String("iPhoneOS".to_string())]),
    );

    // Platform / deployment.
    entries.insert("DTPlatformName".to_string(), Plist::String("iphoneos".to_string()));
    entries.insert("DTPlatformVersion".to_string(), Plist::String("17.0".to_string()));
    entries.insert(
        "MinimumOSVersion".to_string(),
        Plist::String(crate::compile::normalize_ios_version(&app.min_ios_version)),
    );
    entries.insert(
        "UIDeviceFamily".to_string(),
        Plist::Array(app.device_family.iter().map(|value| Plist::Integer(*value)).collect()),
    );
    entries.insert(
        "UIRequiredDeviceCapabilities".to_string(),
        Plist::Array(vec![Plist::String("arm64".to_string())]),
    );
    entries.insert("LSRequiresIPhoneOS".to_string(), Plist::Boolean(true));
    entries.insert(
        "LSApplicationCategoryType".to_string(),
        Plist::String("public.app-category.utilities".to_string()),
    );

    // A modern UIKit app with no storyboard still needs a launch screen, else iOS
    // letterboxes it. An empty UILaunchScreen dict is the documented,
    // storyboard-free way to get a plain launch image.
    entries.insert("UILaunchScreen".to_string(), Plist::Dict(BTreeMap::new()));
    entries.insert(
        "UISupportedInterfaceOrientations".to_string(),
        Plist::Array(vec![
            Plist::String("UIInterfaceOrientationPortrait".to_string()),
            Plist::String("UIInterfaceOrientationLandscapeLeft".to_string()),
            Plist::String("UIInterfaceOrientationLandscapeRight".to_string()),
        ]),
    );

    // Let the user add or override anything, but not the identity keys that the
    // bundle structure itself depends on.
    for key in app.plist_extras.iter().map(|(key, _)| key) {
        if matches!(
            key.as_str(),
            "CFBundleExecutable" | "CFBundleIdentifier" | "CFBundlePackageType"
        ) {
            return Err(Error::Unsupported {
                message: format!(
                    "app.info_plist.{key} cannot be overridden: it must agree with the app \
                     name/bundle id in darwinforge.toml and with the file on disk"
                ),
            });
        }
    }
    for (key, value) in Plist::from_toml(&app.plist_extras)?.as_dict_entries() {
        entries.insert(key, value);
    }

    // The architecture is stamped by the linker in LC_BUILD_VERSION, not here.
    let _ = arch;
    Ok(Plist::Dict(entries))
}

/// Render the `Info.plist` text for an app.
pub fn info_plist_xml(config: &Config, executable_name: &str, arch: &str) -> Result<String> {
    Ok(plist::to_xml(&info_plist(config, executable_name, arch)?))
}

/// Everything the bundle stage produced.
#[derive(Debug)]
pub struct AppBundle {
    pub path: PathBuf,
    pub executable: PathBuf,
    pub resources: Vec<PathBuf>,
}

/// Destination for a resource inside the bundle.
///
/// Files normally land at the bundle root so `pathForResource:ofType:` finds
/// them. Anything inside a `*.lproj` directory keeps that directory, because
/// iOS only looks for localized resources in `<Bundle>.lproj/`.
pub fn resource_destination(app_dir: &Path, resource: &Path) -> PathBuf {
    let components: Vec<String> = resource
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect();
    // Keep everything from the first `.lproj` component onwards.
    let start = components
        .iter()
        .position(|component| component.ends_with(".lproj"))
        .unwrap_or(components.len().saturating_sub(1));
    let mut destination = app_dir.to_path_buf();
    for component in &components[start..] {
        destination.push(component);
    }
    destination
}

/// Assemble `<Name>.app/` with the executable, `Info.plist` and resources.
pub fn assemble(
    config: &Config,
    executable_source: &Path,
    resources: &[PathBuf],
    app_dir: &Path,
    arch: &str,
) -> Result<AppBundle> {
    let bundle_name = config.app_bundle_name();
    let executable_name = config.app.name.clone();
    let executable = app_dir.join(&executable_name);

    // Start from a clean directory so stale files never end up in the .ipa.
    if app_dir.exists() {
        std::fs::remove_dir_all(app_dir)
            .map_err(|source| Error::io(format!("cannot clean {}", app_dir.display()), source))?;
    }
    std::fs::create_dir_all(app_dir)
        .map_err(|source| Error::io(format!("cannot create {}", app_dir.display()), source))?;

    std::fs::copy(executable_source, &executable).map_err(|source| {
        Error::io(
            format!(
                "cannot place the executable {} inside {bundle_name}",
                executable_source.display()
            ),
            source,
        )
    })?;
    set_mode(&executable, is_executable_member(&executable_name))?;

    let plist_xml = info_plist_xml(config, &executable_name, arch)?;
    let plist_path = app_dir.join("Info.plist");
    std::fs::write(&plist_path, plist_xml)
        .map_err(|source| Error::io(format!("cannot write {}", plist_path.display()), source))?;

    let mut copied = Vec::new();
    for resource in resources {
        let destination = resource_destination(app_dir, resource);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io(format!("cannot create {}", parent.display()), source)
            })?;
        }
        std::fs::copy(resource, &destination).map_err(|source| {
            Error::io(
                format!("cannot copy resource {} into {bundle_name}", resource.display()),
                source,
            )
        })?;
        set_mode(&destination, false)?;
        copied.push(destination);
    }

    Ok(AppBundle { path: app_dir.to_path_buf(), executable, resources: copied })
}

/// Whether a member of the bundle must carry the POSIX execute bit.
pub fn is_executable_member(name: &str) -> bool {
    !name.contains('.')
}

/// Set POSIX permissions where the host tracks them. The zip writer always
/// records POSIX modes, so on non-Unix hosts this is a no-op.
pub fn set_mode(path: &Path, executable: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|source| Error::io(format!("cannot chmod {}", path.display()), source))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, executable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let text = r#"
            [app]
        name = "Hello"
        bundle_id = "com.example.hello"
        min_ios_version = "13.0"
        version = "1.2"
        build = "7"
        device_family = [1, 2]

        [app.info_plist]
        ITSAppUsesNonExemptEncryption = false
        "#;
        Config::from_str(text, Path::new("/project")).expect("valid config")
    }

    #[test]
    fn info_plist_has_required_keys() {
        let plist = info_plist(&config(), "Hello", "arm64").expect("builds");
        assert_eq!(
            plist.get("CFBundleIdentifier").and_then(Plist::as_str),
            Some("com.example.hello")
        );
        assert_eq!(plist.get("CFBundleExecutable").and_then(Plist::as_str), Some("Hello"));
        assert_eq!(plist.get("CFBundlePackageType").and_then(Plist::as_str), Some("APPL"));
        assert_eq!(plist.get("MinimumOSVersion").and_then(Plist::as_str), Some("13.0"));
        assert_eq!(plist.get("CFBundleVersion").and_then(Plist::as_str), Some("7"));
        assert_eq!(
            plist.get("CFBundleShortVersionString").and_then(Plist::as_str),
            Some("1.2")
        );
        assert_eq!(
            plist.get("CFBundleSupportedPlatforms").and_then(Plist::as_array),
            Some([Plist::String("iPhoneOS".to_string())].as_slice())
        );
    }

    #[test]
    fn info_plist_device_family_and_capabilities() {
        let plist = info_plist(&config(), "Hello", "arm64").expect("builds");
        let family = plist.get("UIDeviceFamily").and_then(Plist::as_array).expect("array");
        assert_eq!(family, [Plist::Integer(1), Plist::Integer(2)]);
        let capabilities = plist
            .get("UIRequiredDeviceCapabilities")
            .and_then(Plist::as_array)
            .expect("array");
        assert_eq!(capabilities, [Plist::String("arm64".to_string())].as_slice());
    }

    #[test]
    fn info_plist_includes_user_extras_and_launch_screen() {
        let plist = info_plist(&config(), "Hello", "arm64").expect("builds");
        assert_eq!(plist.get("ITSAppUsesNonExemptEncryption"), Some(&Plist::Boolean(false)));
        assert_eq!(plist.get("UILaunchScreen"), Some(&Plist::Dict(BTreeMap::new())));
    }

    #[test]
    fn info_plist_xml_round_trips() {
        let xml = info_plist_xml(&config(), "Hello", "arm64").expect("builds");
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<key>CFBundleIdentifier</key>"));
        let parsed = plist::parse_xml(&xml).expect("valid xml plist");
        assert_eq!(
            parsed.get("CFBundleIdentifier").and_then(Plist::as_str),
            Some("com.example.hello")
        );
    }

    #[test]
    fn info_plist_rejects_identity_overrides() {
        let text = r#"
            [app]
            name = "Hello"
            bundle_id = "com.example.hello"
            min_ios_version = "13.0"

            [app.info_plist]
            CFBundleExecutable = "other"
        "#;
        let config = Config::from_str(text, Path::new("/project")).expect("valid config");
        let error = info_plist(&config, "Hello", "arm64").expect_err("must reject");
        assert!(error.to_string().contains("CFBundleExecutable"));
    }

    #[test]
    fn special_characters_are_escaped() {
        let text = r#"
            [app]
            name = "Hello"
            bundle_id = "com.example.hello"
            min_ios_version = "13.0"
            display_name = "A & B <tag>"
        "#;
        let config = Config::from_str(text, Path::new("/project")).expect("valid config");
        let xml = info_plist_xml(&config, "Hello", "arm64").expect("builds");
        assert!(xml.contains("A &amp; B &lt;tag&gt;"));
        let parsed = plist::parse_xml(&xml).expect("valid xml");
        assert_eq!(
            parsed.get("CFBundleDisplayName").and_then(Plist::as_str),
            Some("A & B <tag>")
        );
    }

    #[test]
    fn executable_bit_detection() {
        assert!(is_executable_member("Hello"));
        assert!(!is_executable_member("Info.plist"));
        assert!(!is_executable_member("logo.png"));
    }
}