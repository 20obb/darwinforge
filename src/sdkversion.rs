//! Reading an SDK's declared version, from whatever it happens to ship.
//!
//! The SDKs distributed by the common third-party repositories carry **both**
//! `SDKSettings.json` and `SDKSettings.plist`, and the plist is not always
//! readable: it may be Apple's binary `bplist00` format rather than XML, which a
//! text-oriented reader cannot parse. A build that only looked at the XML plist
//! therefore reported "unknown" on an SDK that plainly declared its version.
//!
//! Sources are tried in order of trustworthiness, and the first that yields a
//! version wins:
//!
//! 1. `SDKSettings.json` — plain text, always parseable, authoritative.
//! 2. `SDKSettings.plist` — XML, then the binary `bplist00` format.
//! 3. The directory name — `iPhoneOS17.5.sdk` yields `17.5`.
//!
//! Every function takes bytes or a path and returns an `Option`, so all of it is
//! testable without a real SDK.

use std::path::Path;

/// The version of the SDK rooted at `root`, or `None` when nothing declares one.
pub fn read_sdk_version(root: &Path) -> Option<String> {
    from_json_file(root).or_else(|| from_plist_file(root)).or_else(|| from_directory_name(root))
}

/// Read `SDKSettings.json` from disk and extract the version.
pub fn from_json_file(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("SDKSettings.json")).ok()?;
    from_json(&text)
}

/// Extract a version from the flat JSON object in `SDKSettings.json`.
///
/// `Version` is authoritative; `CanonicalName` and `DisplayName` are fallbacks
/// for older files that omit it. Deliberately a minimal reader rather than a
/// JSON parser: the file is machine-generated with a fixed shape, and a real
/// parser would mean a dependency.
pub fn from_json(text: &str) -> Option<String> {
    for key in ["\"Version\"", "\"CanonicalName\"", "\"DisplayName\""] {
        if let Some(value) = json_string_value(text, key) {
            if let Some(version) = from_descriptive_text(&value) {
                return Some(version);
            }
        }
    }
    None
}

/// Extract the string value following `"key":` in a flat JSON document.
pub fn json_string_value(text: &str, key: &str) -> Option<String> {
    let start = text.find(key)? + key.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Read `SDKSettings.plist` from disk, trying XML then binary.
pub fn from_plist_file(root: &Path) -> Option<String> {
    let bytes = std::fs::read(root.join("SDKSettings.plist")).ok()?;
    from_plist_bytes(&bytes)
}

/// Extract a version from plist bytes in either the XML or the binary format.
pub fn from_plist_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(b"bplist00") {
        return from_binary_plist(bytes);
    }
    from_xml_plist(bytes)
}

/// Extract a version from an XML plist, using the crate's own reader.
pub fn from_xml_plist(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let parsed = crate::plist::parse_xml(&text).ok()?;
    for key in ["Version", "CanonicalName", "DisplayName"] {
        if let Some(version) = parsed.get(key).and_then(|v| v.as_str()).and_then(from_descriptive_text)
        {
            return Some(version);
        }
    }
    None
}

/// Pull a bare version out of a descriptive string.
///
/// `CanonicalName` reads like `iPhoneOS17.5` and `DisplayName` like
/// `iOS 17.5 Simulator`; we want the `17.5` in both. Returns `None` when there
/// is no dotted number, so a bare `iPhoneOS17` is not reported as a version.
pub fn from_descriptive_text(text: &str) -> Option<String> {
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0usize;
    while index < characters.len() {
        if !characters[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut dots = 0usize;
        while index < characters.len()
            && (characters[index].is_ascii_digit() || characters[index] == '.')
        {
            dots += usize::from(characters[index] == '.');
            index += 1;
        }
        let candidate: String = characters[start..index].iter().collect();
        let candidate = candidate.trim_end_matches('.');
        // Require a dot so "17" is not mistaken for a version, but allow a
        // trailing component so "17.5.1" survives intact.
        if dots > 0 && candidate.contains('.') {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Last resort: parse the version out of the directory name.
///
/// An SDK unpacked to `iPhoneOS17.5.sdk` declares its version in its own name.
pub fn from_directory_name(root: &Path) -> Option<String> {
    let name = root.file_name()?.to_str()?;
    let stem = name.strip_suffix(".sdk").unwrap_or(name);
    from_descriptive_text(stem)
}

/// Extract a version from a binary `bplist00` plist.
///
/// Rather than a full NeXTSTEP plist parser, this uses the file's own offset
/// table to walk the object list and decode the first string reachable under the
/// key `Version`. It is deliberately narrow: an unexpected shape returns `None`
/// so the directory-name fallback answers instead of a guess being reported as
/// fact.
pub fn from_binary_plist(bytes: &[u8]) -> Option<String> {
    let trailer = bytes.get(bytes.len().checked_sub(32)?..)?;
    // Trailer, big-endian: 5 unused, sortVersion, offsetIntSize, objectRefSize,
    // numObjects, topObject, offsetTableOffset.
    let offset_size = usize::from(trailer[6]);
    let object_ref_size = usize::from(trailer[7]);
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&object_ref_size) {
        return None;
    }
    let read_big_endian = |slice: &[u8]| -> usize {
        slice.iter().fold(0usize, |accumulator, byte| {
            (accumulator << 8) | usize::from(*byte)
        })
    };
    let num_objects = read_big_endian(trailer.get(8..8 + offset_size)?);
    let table_offset = read_big_endian(trailer.get(16..16 + offset_size)?);
    // Guard against a corrupt or hostile file claiming an absurd object count.
    if num_objects == 0 || num_objects > 4096 || table_offset >= bytes.len() {
        return None;
    }

    // The bytes of object `index`, as laid out per the offset table.
    let object_at = |index: usize| -> Option<&[u8]> {
        let start = table_offset.checked_add(index.checked_mul(offset_size)?)?;
        let offset = read_big_endian(bytes.get(start..start + offset_size)?);
        bytes.get(offset..)
    };

    // Find the "Version" key, then decode the next few objects looking for a
    // string. Scanning a bounded window keeps this cheap and avoids treating
    // arbitrary binary as text.
    for index in 0..num_objects {
        let object = object_at(index)?;
        let key = b"Version";
        if !object.starts_with(key) {
            continue;
        }
        for candidate in index + 1..num_objects.min(index + 4) {
            if let Some(object) = object_at(candidate) {
                if let Some(text) = decode_bplist_string(object) {
                    if let Some(version) = from_descriptive_text(&text) {
                        return Some(version);
                    }
                }
            }
        }
        // Also consider bytes immediately following the key itself.
        if let Some(text) = decode_bplist_string(&object[key.len()..]) {
            if let Some(version) = from_descriptive_text(&text) {
                return Some(version);
            }
        }
    }
    None
}

/// Decode a bplist string object.
///
/// Markers `0x5X` and `0x6X` introduce ASCII and UTF-16BE strings of length X.
/// Any other marker is not a string and yields `None`, as does a zero-length
/// string: an empty decode carries no version and would only produce noise.
fn decode_bplist_string(bytes: &[u8]) -> Option<String> {
    let marker = *bytes.first()?;
    let text = match marker >> 4 {
        0x5 => {
            let length = usize::from(marker & 0x0f);
            String::from_utf8_lossy(bytes.get(1..1 + length)?).into_owned()
        }
        0x6 => {
            let length = usize::from(marker & 0x0f) * 2;
            let raw = bytes.get(1..1 + length)?;
            if raw.len() % 2 != 0 {
                return None;
            }
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("darwinforge-sdkver-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A real SDKSettings.json as shipped by the xybp888/iOS-SDKs repository.
    const REAL_JSON: &str = r#"{
  "CanonicalName": "iphonesimulator17.5",
  "DisplayName": "iOS 17.5 Simulator",
  "Version": "17.5",
  "Platform": "iphonesimulator",
  "ProductBuildVersion": "21F79"
}"#;

    #[test]
    fn version_is_read_from_the_json_settings() {
        // The reported bug: the version showed "unknown" although the SDK
        // shipped SDKSettings.json. The JSON is authoritative and must win.
        assert_eq!(from_json(REAL_JSON).as_deref(), Some("17.5"));
    }

    #[test]
    fn json_tolerates_whitespace_and_key_order() {
        let spaced = "{\n  \"Version\"\n:\n    \"17.5\",\n  \"Platform\": \"iphoneos\"\n}";
        assert_eq!(from_json(spaced).as_deref(), Some("17.5"));
    }

    #[test]
    fn json_falls_back_to_canonical_name_when_version_is_absent() {
        let text = r#"{"CanonicalName": "iPhoneOS17.5", "Platform": "iphoneos"}"#;
        assert_eq!(from_json(text).as_deref(), Some("17.5"));
    }

    #[test]
    fn json_without_any_version_yields_none() {
        assert_eq!(from_json(r#"{"Platform": "iphoneos"}"#), None);
        assert_eq!(from_json("{}"), None);
    }

    #[test]
    fn version_is_read_from_an_xml_plist() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CanonicalName</key><string>iPhoneOS17.5</string>
  <key>Version</key><string>17.5</string>
</dict></plist>"#;
        assert_eq!(from_plist_bytes(xml).as_deref(), Some("17.5"));
    }

    #[test]
    fn descriptive_strings_yield_the_bare_version() {
        for (input, expected) in [
            ("iPhoneOS17.5", "17.5"),
            ("iOS 17.5 Simulator", "17.5"),
            ("17.5", "17.5"),
            ("17.5.1", "17.5.1"),
            ("iPhoneOS18.0", "18.0"),
        ] {
            assert_eq!(
                from_descriptive_text(input).as_deref(),
                Some(expected),
                "input {input:?}"
            );
        }
        // A bare major version is not a version we should claim, and a trailing
        // dot must not be reported as one either.
        for input in ["iPhoneOS17", "iPhoneOS", "", "no digits here", "17."] {
            assert_eq!(from_descriptive_text(input), None, "input {input:?}");
        }
    }

    #[test]
    fn the_directory_name_is_the_last_resort() {
        assert_eq!(
            from_directory_name(std::path::Path::new("/opt/iPhoneOS17.5.sdk")).as_deref(),
            Some("17.5")
        );
        assert_eq!(
            from_directory_name(std::path::Path::new("/opt/iPhoneOS18.0.sdk")).as_deref(),
            Some("18.0")
        );
        assert_eq!(from_directory_name(std::path::Path::new("/opt/SomeSDK")), None);
    }

    #[test]
    fn json_wins_over_the_directory_name() {
        let dir = scratch("priority");
        let sdk = dir.join("iPhoneOS16.4.sdk");
        std::fs::create_dir_all(&sdk).expect("mkdir");
        // The directory claims 16.4 but the settings say 17.5: settings win.
        std::fs::write(sdk.join("SDKSettings.json"), REAL_JSON).expect("write");
        assert_eq!(read_sdk_version(&sdk).as_deref(), Some("17.5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plist_is_used_when_no_json_is_present() {
        let dir = scratch("plist");
        let sdk = dir.join("iPhoneOS17.5.sdk");
        std::fs::create_dir_all(&sdk).expect("mkdir");
        std::fs::write(
            sdk.join("SDKSettings.plist"),
            "<plist version=\"1.0\"><dict><key>Version</key><string>17.5</string></dict></plist>",
        )
        .expect("write");
        assert_eq!(read_sdk_version(&sdk).as_deref(), Some("17.5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_sdk_with_no_settings_at_all_falls_back_to_its_name() {
        let dir = scratch("dirname");
        let sdk = dir.join("iPhoneOS17.5.sdk");
        std::fs::create_dir_all(&sdk).expect("mkdir");
        assert_eq!(read_sdk_version(&sdk).as_deref(), Some("17.5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_sdk_that_declares_nothing_reports_none() {
        let dir = scratch("unknown");
        let sdk = dir.join("MysterySDK");
        std::fs::create_dir_all(&sdk).expect("mkdir");
        assert_eq!(read_sdk_version(&sdk), None, "no version must mean None, not a guess");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_json_does_not_stop_the_plist_from_being_tried() {
        let dir = scratch("corrupt");
        let sdk = dir.join("iPhoneOS17.5.sdk");
        std::fs::create_dir_all(&sdk).expect("mkdir");
        std::fs::write(sdk.join("SDKSettings.json"), "{ this is not json").expect("write");
        std::fs::write(
            sdk.join("SDKSettings.plist"),
            "<plist version=\"1.0\"><dict><key>Version</key><string>17.5</string></dict></plist>",
        )
        .expect("write");
        assert_eq!(read_sdk_version(&sdk).as_deref(), Some("17.5"));
        let _ = std::fs::remove_dir_all(&dir);
    }
#[test]
    fn a_truncated_binary_plist_is_rejected_rather_than_guessed() {
        // Corrupt input must return None, never a fabricated version.
        assert_eq!(from_binary_plist(b"bplist00"), None);
        assert_eq!(from_binary_plist(b""), None);
        assert_eq!(from_binary_plist(b"not a plist at all"), None);
        assert_eq!(from_binary_plist(&[0u8; 8]), None);
    }

    #[test]
    fn a_binary_plist_with_an_absurd_object_count_is_rejected() {
        // Defensive: a corrupt trailer must not cause a huge scan.
        let mut bytes = b"bplist00".to_vec();
        bytes.extend_from_slice(&[0u8; 24]);
        bytes[6] = 1; // offsetIntSize
        bytes[7] = 1; // objectRefSize
        bytes.push(0xFF); // absurd numObjects high byte
        assert_eq!(from_binary_plist(&bytes), None);
    }

    #[test]
    fn utf16_strings_decode_from_binary_plists() {
        // "17.5" as a bplist UTF-16BE string: marker 0x64, four units.
        let mut encoded = vec![0x64u8];
        for unit in "17.5".encode_utf16() {
            encoded.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_bplist_string(&encoded).as_deref(), Some("17.5"));
    }

    #[test]
    fn ascii_strings_decode_from_binary_plists() {
        let mut encoded = vec![0x53u8]; // 0x5X with length 3
        encoded.extend_from_slice(b"abc");
        assert_eq!(decode_bplist_string(&encoded).as_deref(), Some("abc"));
    }

    #[test]
    fn non_string_markers_are_not_decoded() {
        assert_eq!(decode_bplist_string(&[0xA1, 0x00, 0x00]), None, "array marker");
        assert_eq!(decode_bplist_string(&[]), None);
        // 0x60 declares one UTF-16 unit but only one byte follows it.
        assert_eq!(decode_bplist_string(&[0x60]), None, "truncated UTF-16 string");
    }
}