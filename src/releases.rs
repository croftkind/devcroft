//! Quiet, bounded checks for newer stable GitHub releases. Never installs code.
use std::process::{Command, Stdio};

pub(crate) const INSTALL_COMMAND: &str = "curl -fsSL https://github.com/croftkind/devcroft/releases/latest/download/install.sh | bash";
#[cfg(any(target_os = "macos", test))]
const HOMEBREW_COMMAND: &str = "brew upgrade --cask croftkind/tap/devcroft";
const API_URL: &str = "https://api.github.com/repos/croftkind/devcroft/releases/latest";
const RELEASE_URL: &str = "https://github.com/croftkind/devcroft/releases/tag/";

/// System packages are upgraded through apt after downloading the new .deb.
/// Their update action opens the release assets instead of copying an installer
/// that would create a second, user-local installation.
pub(crate) fn uses_system_package() -> bool {
    cfg!(target_os = "linux")
        && std::env::current_exe()
            .is_ok_and(|path| path == std::path::Path::new("/usr/bin/devcroft"))
}

/// Finder does not normally inherit Homebrew's PATH. Match the running bundle
/// against Homebrew's installed cask links without invoking brew on the UI thread.
pub(crate) fn update_command() -> &'static str {
    #[cfg(target_os = "macos")]
    if let Ok(executable) = std::env::current_exe() {
        let mut caskrooms = vec![
            std::path::PathBuf::from("/opt/homebrew/Caskroom"),
            std::path::PathBuf::from("/usr/local/Caskroom"),
        ];
        if let Some(prefix) = std::env::var_os("HOMEBREW_PREFIX") {
            caskrooms.push(std::path::PathBuf::from(prefix).join("Caskroom"));
        }
        return command_for_install(&executable, &caskrooms);
    }
    INSTALL_COMMAND
}

#[cfg(any(target_os = "macos", test))]
fn command_for_install(
    executable: &std::path::Path,
    caskrooms: &[std::path::PathBuf],
) -> &'static str {
    let Ok(executable) = executable.canonicalize() else {
        return INSTALL_COMMAND;
    };
    let Some(bundle) = executable.ancestors().nth(3) else {
        return INSTALL_COMMAND;
    };
    if bundle.file_name().is_none_or(|name| name != "Devcroft.app") {
        return INSTALL_COMMAND;
    }
    let Ok(bundle) = bundle.canonicalize() else {
        return INSTALL_COMMAND;
    };
    for caskroom in caskrooms {
        let Ok(versions) = std::fs::read_dir(caskroom.join("devcroft")) else {
            continue;
        };
        for entry in versions.flatten() {
            if entry
                .path()
                .join("Devcroft.app")
                .canonicalize()
                .ok()
                .as_ref()
                == Some(&bundle)
            {
                return HOMEBREW_COMMAND;
            }
        }
    }
    INSTALL_COMMAND
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Release {
    pub(crate) tag: String,
}

impl Release {
    pub(crate) fn url(&self) -> String {
        format!("{RELEASE_URL}{}", self.tag)
    }
}

// Strict stable versions match the release branch contract. Numeric comparison
// avoids lexicographic mistakes such as treating 0.10.0 as older than 0.9.0.
fn version(value: &str) -> Option<[u64; 3]> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let mut result = [0; 3];
    for (index, part) in parts.into_iter().enumerate() {
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        result[index] = part.parse().ok()?;
    }
    Some(result)
}

fn newer_release(body: &[u8], current: &str) -> Option<Release> {
    #[derive(serde::Deserialize)]
    struct Response {
        tag_name: String,
        draft: bool,
        prerelease: bool,
    }
    let response: Response = serde_json::from_slice(body).ok()?;
    let latest = version(response.tag_name.strip_prefix('v')?)?;
    if response.draft || response.prerelease || latest <= version(current)? {
        return None;
    }
    Some(Release {
        tag: response.tag_name,
    })
}

pub(crate) fn check() -> Option<Release> {
    // The test platform must never depend on public network access.
    if cfg!(test) || std::env::var_os("DEVCROFT_DISABLE_UPDATE_CHECK").is_some() {
        return None;
    }
    let output = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "10",
            "--max-filesize",
            "1048576",
            "--header",
            "Accept: application/vnd.github+json",
            "--user-agent",
            concat!("Devcroft/", env!("CARGO_PKG_VERSION")),
            API_URL,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 1_048_576 {
        return None;
    }
    newer_release(&output.stdout, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn homebrew_updates_only_the_bundle_owned_by_the_cask() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Applications/Devcroft.app");
        let executable = bundle.join("Contents/MacOS/devcroft");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "binary").unwrap();
        let caskroom = root.path().join("Caskroom");
        let cask = caskroom.join("devcroft/1.2.3/Devcroft.app");
        std::fs::create_dir_all(cask.parent().unwrap()).unwrap();
        let roots = [caskroom];
        assert_eq!(command_for_install(&executable, &roots), INSTALL_COMMAND);
        std::os::unix::fs::symlink(&bundle, &cask).unwrap();
        assert_eq!(command_for_install(&executable, &roots), HOMEBREW_COMMAND);
        let cli_link = root.path().join("bin/devcroft");
        std::fs::create_dir_all(cli_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&executable, &cli_link).unwrap();
        assert_eq!(command_for_install(&cli_link, &roots), HOMEBREW_COMMAND);
        let separate = root
            .path()
            .join("Other/Devcroft.app/Contents/MacOS/devcroft");
        std::fs::create_dir_all(separate.parent().unwrap()).unwrap();
        std::fs::write(&separate, "separate binary").unwrap();
        assert_eq!(command_for_install(&separate, &roots), INSTALL_COMMAND);
        assert_eq!(
            command_for_install(std::path::Path::new("/usr/bin/devcroft"), &roots),
            INSTALL_COMMAND
        );
    }

    fn response(tag: &str, draft: bool, prerelease: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": prerelease,
            "html_url": "https://untrusted.example/ignored"
        }))
        .unwrap()
    }

    #[test]
    fn only_newer_stable_releases_are_offered() {
        let release = newer_release(&response("v0.10.0", false, false), "0.9.9").unwrap();
        assert_eq!(release.url(), format!("{RELEASE_URL}v0.10.0"));
        for tag in [
            "v0.9.9",
            "v0.9.0",
            "v0.10.0-beta.1",
            "v01.0.0",
            "v1.2",
            "1.2.3",
            "v1.2.3/path",
        ] {
            assert!(
                newer_release(&response(tag, false, false), "0.9.9").is_none(),
                "{tag}"
            );
        }
        assert!(newer_release(&response("v1.0.0", true, false), "0.9.9").is_none());
        assert!(newer_release(&response("v1.0.0", false, true), "0.9.9").is_none());
        assert!(newer_release(b"not json", "0.9.9").is_none());
        assert!(newer_release(br#"{"message":"API rate limit exceeded"}"#, "0.9.9").is_none());
    }

    #[test]
    fn stable_versions_compare_components_and_reject_malformed_values() {
        assert!(version("10.0.0") > version("9.99.99"));
        for value in [
            "1.2.3.4",
            "1.2.-3",
            "1.2.+3",
            "1.2.",
            " 1.2.3",
            "1.2.03",
            "18446744073709551616.0.0",
        ] {
            assert!(version(value).is_none(), "{value}");
        }
    }
}
