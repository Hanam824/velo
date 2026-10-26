//! Telling browsers where Velo's native-messaging host is, so its extension can talk to it.
//!
//! Each browser looks for a JSON manifest named after the host: Chromium-based browsers and
//! Firefox each in their own folders on macOS and Linux, and through registry keys under
//! `HKEY_CURRENT_USER` that point at the manifest on Windows. Registering again overwrites the
//! same files, so the app does it at every start; it then also follows the app when it moves.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Result;

/// The name browsers know Velo's host by.
pub const NATIVE_HOST_NAME: &str = "io.github.hanam824.velo";

/// The ID of Velo's Chromium extension, fixed by the public key in its manifest.
pub const CHROMIUM_EXTENSION_ID: &str = "knncpmilkobjkfcjafpleodokkhcaing";

/// The ID of Velo's Firefox add-on, from `browser_specific_settings` in its manifest.
pub const FIREFOX_EXTENSION_ID: &str = "velo@hanam824.github.io";

/// What the host is and which extensions may start it.
#[derive(Debug, Clone, Copy)]
pub struct NativeHost<'a> {
    /// The `velo-nmh` program.
    pub path: &'a Path,
    /// The IDs of the Chromium extensions allowed to use it.
    pub chromium_extensions: &'a [&'a str],
    /// The IDs of the Firefox add-ons allowed to use it.
    pub firefox_extensions: &'a [&'a str],
}

/// The two families of manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Chromium,
    Firefox,
}

impl<'a> NativeHost<'a> {
    /// Velo's host at `path`, for Velo's own extensions.
    pub fn velo(path: &'a Path) -> Self {
        Self {
            path,
            chromium_extensions: &[CHROMIUM_EXTENSION_ID],
            firefox_extensions: &[FIREFOX_EXTENSION_ID],
        }
    }

    /// The manifest for `family`.
    fn manifest(&self, family: Family) -> String {
        let mut manifest = json!({
            "name": NATIVE_HOST_NAME,
            "description": "Velo Download Manager",
            "path": self.path,
            "type": "stdio",
        });
        match family {
            Family::Chromium => {
                manifest["allowed_origins"] = self
                    .chromium_extensions
                    .iter()
                    .map(|id| format!("chrome-extension://{id}/"))
                    .collect();
            }
            Family::Firefox => {
                manifest["allowed_extensions"] = json!(self.firefox_extensions);
            }
        }
        serde_json::to_string_pretty(&manifest).expect("a manifest serializes")
    }
}

/// Registers `host` with the browsers installed for this user and returns the manifests
/// written. A browser that isn't installed is skipped (on Windows, every browser's key is
/// written, which is harmless).
pub fn register_native_host(host: &NativeHost<'_>, data_dir: &Path) -> Result<Vec<PathBuf>> {
    let home = etcetera::home_dir().map_err(|_| crate::Error::NoHome)?;
    register_in(host, &home, data_dir)
}

/// [`register_native_host`] for the user whose home folder is `home`.
fn register_in(host: &NativeHost<'_>, home: &Path, data_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (family, dir) in manifest_dirs(home, data_dir) {
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{NATIVE_HOST_NAME}.json"));
        std::fs::write(&path, host.manifest(family))?;
        register_path(family, &path)?;
        written.push(path);
    }
    Ok(written)
}

/// The folders to put manifests in, for the browsers installed under `home`.
#[cfg(target_os = "macos")]
fn manifest_dirs(home: &Path, _data_dir: &Path) -> Vec<(Family, PathBuf)> {
    let support = home.join("Library/Application Support");
    // Each browser's profile folder, which shows it is installed, and where its hosts go.
    let browsers = [
        (Family::Chromium, "Google/Chrome", "Google/Chrome"),
        (Family::Chromium, "Chromium", "Chromium"),
        (Family::Chromium, "Microsoft Edge", "Microsoft Edge"),
        (
            Family::Chromium,
            "BraveSoftware/Brave-Browser",
            "BraveSoftware/Brave-Browser",
        ),
        (Family::Chromium, "Vivaldi", "Vivaldi"),
        (Family::Firefox, "Firefox", "Mozilla"),
    ];
    browsers
        .into_iter()
        .filter(|(_, profile, _)| support.join(profile).is_dir())
        .map(|(family, _, hosts)| (family, support.join(hosts).join("NativeMessagingHosts")))
        .collect()
}

/// The folders to put manifests in, for the browsers installed under `home`.
#[cfg(all(unix, not(target_os = "macos")))]
fn manifest_dirs(home: &Path, _data_dir: &Path) -> Vec<(Family, PathBuf)> {
    let config = home.join(".config");
    let mut dirs: Vec<(Family, PathBuf)> = [
        "google-chrome",
        "chromium",
        "microsoft-edge",
        "BraveSoftware/Brave-Browser",
        "vivaldi",
    ]
    .into_iter()
    .filter(|dir| config.join(dir).is_dir())
    .map(|dir| {
        (
            Family::Chromium,
            config.join(dir).join("NativeMessagingHosts"),
        )
    })
    .collect();
    if home.join(".mozilla").is_dir() {
        dirs.push((
            Family::Firefox,
            home.join(".mozilla/native-messaging-hosts"),
        ));
    }
    dirs
}

/// The folders to put manifests in: Velo's own on Windows, where registry keys point at them.
#[cfg(windows)]
fn manifest_dirs(_home: &Path, data_dir: &Path) -> Vec<(Family, PathBuf)> {
    vec![
        (
            Family::Chromium,
            data_dir.join("native-host").join("chromium"),
        ),
        (
            Family::Firefox,
            data_dir.join("native-host").join("firefox"),
        ),
    ]
}

/// Nothing to do outside Windows: browsers find manifests by their folder.
#[cfg(not(windows))]
fn register_path(_family: Family, _manifest: &Path) -> Result<()> {
    Ok(())
}

/// Points the browsers of `family` at `manifest` through their registry keys.
#[cfg(windows)]
fn register_path(family: Family, manifest: &Path) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let keys: &[&str] = match family {
        Family::Chromium => &[
            r"Software\Google\Chrome\NativeMessagingHosts",
            r"Software\Chromium\NativeMessagingHosts",
            r"Software\Microsoft\Edge\NativeMessagingHosts",
            r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        ],
        Family::Firefox => &[r"Software\Mozilla\NativeMessagingHosts"],
    };
    let user = RegKey::predef(HKEY_CURRENT_USER);
    for key in keys {
        let (host, _) = user.create_subkey(format!(r"{key}\{NATIVE_HOST_NAME}"))?;
        host.set_value("", &manifest.as_os_str())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> NativeHost<'static> {
        NativeHost {
            path: Path::new("/opt/velo/velo-nmh"),
            chromium_extensions: &["knncpmilkobjkfcjafpleodokkhcaing"],
            firefox_extensions: &["velo@hanam824.github.io"],
        }
    }

    #[test]
    fn manifests_name_the_host_and_the_allowed_extensions() {
        let chromium: serde_json::Value =
            serde_json::from_str(&host().manifest(Family::Chromium)).unwrap();
        assert_eq!(chromium["name"], NATIVE_HOST_NAME);
        assert_eq!(chromium["type"], "stdio");
        assert_eq!(chromium["path"], "/opt/velo/velo-nmh");
        assert_eq!(
            chromium["allowed_origins"],
            json!(["chrome-extension://knncpmilkobjkfcjafpleodokkhcaing/"])
        );
        let firefox: serde_json::Value =
            serde_json::from_str(&host().manifest(Family::Firefox)).unwrap();
        assert_eq!(
            firefox["allowed_extensions"],
            json!(["velo@hanam824.github.io"])
        );
        assert!(firefox.get("allowed_origins").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn only_installed_browsers_get_a_manifest() {
        let home = tempfile::tempdir().unwrap();
        // Firefox's profile folder, and where its hosts go.
        let (chrome, firefox, firefox_hosts) = if cfg!(target_os = "macos") {
            (
                "Library/Application Support/Google/Chrome",
                "Library/Application Support/Firefox",
                "Library/Application Support/Mozilla/NativeMessagingHosts",
            )
        } else {
            (
                ".config/google-chrome",
                ".mozilla",
                ".mozilla/native-messaging-hosts",
            )
        };
        std::fs::create_dir_all(home.path().join(chrome)).unwrap();
        std::fs::create_dir_all(home.path().join(firefox)).unwrap();
        let written = register_in(&host(), home.path(), home.path()).unwrap();
        assert_eq!(written.len(), 2, "{written:?}");
        let name = format!("{NATIVE_HOST_NAME}.json");
        assert!(written[0].starts_with(home.path().join(chrome)));
        assert!(written[0].ends_with(&name));
        assert!(written[1].starts_with(home.path().join(firefox_hosts)));
        let text = std::fs::read_to_string(&written[1]).unwrap();
        assert!(text.contains("allowed_extensions"));
        // Registering again overwrites the same files.
        assert_eq!(
            register_in(&host(), home.path(), home.path()).unwrap(),
            written
        );
    }
}
