//! Self-update for OpenSuperCAD from GitHub Releases.
//!
//! Downloads go through the system `curl` (HTTPS only) and archives are
//! unpacked with `tar`, like the rest of OpenSuperCAD drives `openscad` and
//! `git`. Only immutable releases are used: GitHub locks their tag and
//! assets once published, so files can't be swapped afterwards. Nothing is
//! installed unless the download's SHA-256 matches both the release's
//! `SHA256SUMS` and the digest GitHub records for the asset, and
//! `SHA256SUMS` itself matches its own GitHub digest.
//!
//! How an update is applied depends on how OpenSuperCAD was installed:
//! a tarball in a writable folder is replaced in place (keeping `.old`
//! copies), macOS gets the verified `.dmg` opened, and package-manager
//! installs (deb, Arch, Homebrew) are told which command to run.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

pub mod node;
pub mod openscad;
use sha2::{Digest, Sha256};

/// The repository releases come from.
pub const REPO: &str = "1ARdotNO/OpenSuperCAD";

/// Binaries shipped in the release archives.
pub const BINARIES: [&str; 2] = ["opensupercad", "opensupercad-mcp"];

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("could not run {tool}: {source}")]
    Spawn {
        tool: &'static str,
        source: std::io::Error,
    },
    #[error("download of {url} failed: {message}")]
    Download { url: String, message: String },
    #[error("unexpected release data: {0}")]
    Release(String),
    #[error("{0} has no asset for this platform")]
    NoAsset(String),
    #[error("checksum mismatch for {name}: expected {expected}, got {actual}")]
    Checksum {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("{0} is not listed in SHA256SUMS")]
    NotInSums(String),
    #[error(
        "release {0} is not immutable on GitHub, so its files could have been replaced; \
         not updating from it"
    )]
    NotImmutable(String),
    #[error("GitHub reports no SHA-256 digest for {0}; not installing it")]
    NoDigest(String),
    #[error("unpacking {0} failed")]
    Unpack(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

/// A semantic version `major.minor.patch`; anything after `-`/`+` is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let core = s.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        let v = Version(it.next()??, it.next().flatten()?, it.next().flatten()?);
        Some(v)
    }

    /// The version of the running build.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// The SHA-256 GitHub records for the asset (`digest: sha256:<hex>`).
    /// Required for anything we install, see [`verify`].
    pub sha256: Option<String>,
}

impl Release {
    /// Parse the GitHub "latest release" API response.
    pub fn from_github_json(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            tag_name: String,
            html_url: String,
            #[serde(default)]
            draft: bool,
            #[serde(default)]
            prerelease: bool,
            /// GitHub's immutable releases: tag and assets are locked.
            #[serde(default)]
            immutable: bool,
            assets: Vec<RawAsset>,
        }
        #[derive(Deserialize)]
        struct RawAsset {
            name: String,
            browser_download_url: String,
            digest: Option<String>,
        }
        let raw: Raw =
            serde_json::from_str(json).map_err(|e| UpdateError::Release(e.to_string()))?;
        if raw.draft || raw.prerelease {
            return Err(UpdateError::Release(format!(
                "{} is not a final release",
                raw.tag_name
            )));
        }
        if !raw.immutable {
            return Err(UpdateError::NotImmutable(raw.tag_name));
        }
        let version = Version::parse(&raw.tag_name)
            .ok_or_else(|| UpdateError::Release(format!("bad tag {}", raw.tag_name)))?;
        Ok(Release {
            version,
            tag: raw.tag_name,
            html_url: raw.html_url,
            assets: raw
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                    sha256: a
                        .digest
                        .and_then(|d| d.strip_prefix("sha256:").map(str::to_ascii_lowercase)),
                })
                .collect(),
        })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    pub fn is_newer_than(&self, version: Version) -> bool {
        self.version > version
    }
}

/// How the running copy was installed, which decides how to update it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// A release tarball unpacked into a folder we can write to.
    Tarball { dir: PathBuf },
    /// A `.deb` or pacman package from our releases: we download the new
    /// one; installing it needs `sudo`, so the user runs the command.
    LinuxPackage { format: PackageFormat },
    /// Installed by a package manager; the user updates with `command`.
    Package {
        manager: &'static str,
        command: &'static str,
    },
    /// The macOS app bundle: we open the new `.dmg`.
    MacApp { bundle: PathBuf },
    /// Installed by the Windows installer: we run the new, verified one.
    WindowsInstaller { dir: PathBuf },
    /// A development build or a read-only location.
    Unsupported { reason: String },
}

/// The Linux packages we publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageFormat {
    Deb,
    Pacman,
}

impl PackageFormat {
    /// The format the system's package manager installs, if it is one of ours.
    fn of_this_system() -> Option<Self> {
        if Path::new("/usr/bin/pacman").exists() || Path::new("/etc/arch-release").exists() {
            Some(Self::Pacman)
        } else if Path::new("/usr/bin/dpkg").exists() {
            Some(Self::Deb)
        } else {
            None
        }
    }

    /// The command that installs the downloaded `package`.
    pub fn install_command(self, package: &Path) -> String {
        let quoted = format!("'{}'", package.to_string_lossy().replace('\'', r"'\''"));
        match self {
            Self::Deb => format!("sudo apt install {quoted}"),
            Self::Pacman => format!("sudo pacman -U {quoted}"),
        }
    }
}

impl Install {
    /// Work out how `exe` (normally `std::env::current_exe()`) was installed.
    pub fn detect(exe: &Path) -> Self {
        // Forward slashes, so the checks below also match Windows paths.
        let s = exe.to_string_lossy().replace('\\', "/");
        if s.contains("/Cellar/") || s.contains("/Caskroom/") || s.starts_with("/opt/homebrew/") {
            return Install::Package {
                manager: "Homebrew",
                command: "brew upgrade --cask opensupercad",
            };
        }
        if let Some(bundle) = exe
            .ancestors()
            .find(|a| a.extension().is_some_and(|e| e == "app"))
        {
            return Install::MacApp {
                bundle: bundle.to_path_buf(),
            };
        }
        if s.starts_with("/usr/") && !s.starts_with("/usr/local/") {
            return match PackageFormat::of_this_system() {
                Some(format) => Install::LinuxPackage { format },
                None => Install::Package {
                    manager: "your package manager",
                    command: "update the opensupercad package with it",
                },
            };
        }
        if s.contains("/target/debug/") || s.contains("/target/release/") {
            return Install::Unsupported {
                reason: "this is a development build; update with git pull and cargo build".into(),
            };
        }
        // Inno Setup leaves its uninstaller next to the files it installed.
        if let Some(dir) = exe.parent().filter(|d| d.join("unins000.exe").is_file()) {
            return Install::WindowsInstaller {
                dir: dir.to_path_buf(),
            };
        }
        match exe.parent() {
            Some(dir) if is_writable(dir) => Install::Tarball {
                dir: dir.to_path_buf(),
            },
            Some(dir) => Install::Unsupported {
                reason: format!(
                    "{} is not writable; run `sudo opensupercad update`",
                    dir.display()
                ),
            },
            None => Install::Unsupported {
                reason: "cannot locate the executable".into(),
            },
        }
    }
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".opensupercad-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The release asset to download for `install` on this OS/architecture.
pub fn asset_name(version: Version, install: &Install, os: &str, arch: &str) -> Option<String> {
    match (install, os) {
        (Install::Tarball { .. }, "linux") if matches!(arch, "x86_64" | "aarch64") => {
            Some(format!("opensupercad-{version}-{arch}-linux.tar.gz"))
        }
        (Install::Tarball { .. }, "macos") => {
            Some(format!("opensupercad-{version}-macos-universal.tar.gz"))
        }
        (Install::MacApp { .. }, "macos") => {
            Some(format!("OpenSuperCAD-{version}-macos-universal.dmg"))
        }
        (Install::LinuxPackage { format }, "linux") => match (format, arch) {
            (PackageFormat::Deb, "x86_64") => Some(format!("opensupercad_{version}-1_amd64.deb")),
            (PackageFormat::Deb, "aarch64") => Some(format!("opensupercad_{version}-1_arm64.deb")),
            (PackageFormat::Pacman, "x86_64") => {
                Some(format!("opensupercad-{version}-1-x86_64.pkg.tar.zst"))
            }
            _ => None,
        },
        (Install::WindowsInstaller { .. }, "windows") if arch == "x86_64" => {
            Some(format!("OpenSuperCAD-{version}-windows-x86_64-setup.exe"))
        }
        _ => None,
    }
}

/// Parse `sha256sum` output: `<hex>  <name>` per line.
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.split_once(char::is_whitespace)?;
            let name = name.trim_start().trim_start_matches('*');
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Check `file` against the release's checksums: the `SHA256SUMS` entry is
/// required, and GitHub's own digest must agree when it is reported.
pub fn verify(file: &Path, asset: &Asset, sums: &HashMap<String, String>) -> Result<()> {
    let expected = sums
        .get(&asset.name)
        .ok_or_else(|| UpdateError::NotInSums(asset.name.clone()))?;
    let digest = asset
        .sha256
        .as_ref()
        .ok_or_else(|| UpdateError::NoDigest(asset.name.clone()))?;
    let actual = sha256_file(file)?;
    for want in [expected, digest] {
        if *want != actual {
            return Err(UpdateError::Checksum {
                name: asset.name.clone(),
                expected: want.clone(),
                actual,
            });
        }
    }
    Ok(())
}

/// `bytes` (a small file read into memory, like `SHA256SUMS`) must match
/// the digest GitHub records for `asset`.
fn check_digest(bytes: &[u8], asset: &Asset) -> Result<()> {
    let want = asset
        .sha256
        .as_ref()
        .ok_or_else(|| UpdateError::NoDigest(asset.name.clone()))?;
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if *want != actual {
        return Err(UpdateError::Checksum {
            name: asset.name.clone(),
            expected: want.clone(),
            actual,
        });
    }
    Ok(())
}

/// Talks to GitHub through `curl`.
#[derive(Debug, Clone)]
pub struct Client {
    pub repo: String,
    pub curl: PathBuf,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            repo: REPO.into(),
            curl: "curl".into(),
        }
    }
}

impl Client {
    pub fn latest(&self) -> Result<Release> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", self.repo);
        let body = self.get(&url, None)?;
        Release::from_github_json(&String::from_utf8_lossy(&body))
    }

    /// Download `asset` into `dir` and verify it. Returns the file path.
    pub fn download_verified(
        &self,
        release: &Release,
        asset: &Asset,
        dir: &Path,
    ) -> Result<PathBuf> {
        let sums_asset = release
            .asset("SHA256SUMS")
            .ok_or_else(|| UpdateError::NoAsset("SHA256SUMS".into()))?;
        let sums = self.get(&sums_asset.url, None)?;
        check_digest(&sums, sums_asset)?;
        let sums = parse_sums(&String::from_utf8_lossy(&sums));
        let file = dir.join(&asset.name);
        self.get(&asset.url, Some(&file))?;
        if let Err(e) = verify(&file, asset, &sums) {
            let _ = std::fs::remove_file(&file);
            return Err(e);
        }
        Ok(file)
    }

    /// Download `url` into `out`, calling `progress` with the bytes received
    /// so far while it runs. Verify the file before using it.
    pub fn download(&self, url: &str, out: &Path, mut progress: impl FnMut(u64)) -> Result<()> {
        let mut cmd = self.curl_command(url, Some(out));
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|source| UpdateError::Spawn {
            tool: "curl",
            source,
        })?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            progress(std::fs::metadata(out).map(|m| m.len()).unwrap_or(0));
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        if !status.success() {
            let mut message = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut message);
            }
            return Err(UpdateError::Download {
                url: url.into(),
                message: message.trim().to_owned(),
            });
        }
        progress(std::fs::metadata(out).map(|m| m.len()).unwrap_or(0));
        Ok(())
    }

    /// GET `url` over HTTPS only, into `out` or into memory.
    fn get(&self, url: &str, out: Option<&Path>) -> Result<Vec<u8>> {
        let output = self
            .curl_command(url, out)
            .output()
            .map_err(|source| UpdateError::Spawn {
                tool: "curl",
                source,
            })?;
        if !output.status.success() {
            return Err(UpdateError::Download {
                url: url.into(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(output.stdout)
    }

    /// `curl` for `url` (saved to `out`, if given), refusing anything but
    /// HTTPS, redirects included.
    fn curl_command(&self, url: &str, out: Option<&Path>) -> Command {
        let mut cmd = Command::new(&self.curl);
        cmd.args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            // Generous: OpenSCAD builds are 50-85 MB.
            "--max-time",
            "1800",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            concat!("opensupercad/", env!("CARGO_PKG_VERSION")),
        ]);
        if let Some(out) = out {
            cmd.arg("--output").arg(out);
        }
        // The URL goes last, after `--`, so it can never act as an option.
        cmd.arg("--").arg(url);
        cmd
    }
}

/// Start a verified Windows installer, silently, and return straight away.
/// It closes the running app (Restart Manager) and starts the new version
/// when it's done (`/relaunch=1`, see `packaging/windows/opensupercad.iss`).
pub fn run_windows_installer(installer: &Path) -> Result<()> {
    Command::new(installer)
        .args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/relaunch=1",
        ])
        .spawn()
        .map_err(|source| UpdateError::Spawn {
            tool: "the installer",
            source,
        })?;
    Ok(())
}

/// Unpack a verified release tarball and replace the binaries in `dir`.
///
/// The new files are unpacked next to the old ones first, so the swap is a
/// rename on the same file system. The previous binaries are kept as
/// `<name>.old` for rollback. Returns the replaced paths.
pub fn install_tarball(archive: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
    let staging = tempfile_dir(dir)?;
    let status = Command::new("tar")
        .arg("--no-same-owner")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|source| UpdateError::Spawn {
            tool: "tar",
            source,
        })?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(UpdateError::Unpack(archive.display().to_string()));
    }
    // Archives hold one top-level folder (`opensupercad-<v>-<target>/`).
    let root = std::fs::read_dir(&staging)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_dir())
        .unwrap_or_else(|| staging.clone());
    let mut replaced = Vec::new();
    for name in BINARIES {
        let new = root.join(name);
        if !new.is_file() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(UpdateError::Unpack(format!(
                "{name} missing from the archive"
            )));
        }
        let target = dir.join(name);
        if target.exists() {
            std::fs::rename(&target, dir.join(format!("{name}.old")))?;
        }
        std::fs::rename(&new, &target)?;
        replaced.push(target);
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(replaced)
}

fn tempfile_dir(dir: &Path) -> Result<PathBuf> {
    let path = dir.join(format!(".opensupercad-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{
        Asset, Install, PackageFormat, Release, UpdateError, Version, asset_name, check_digest,
        install_tarball, parse_sums, sha256_file, verify,
    };
    use std::path::Path;

    const JSON: &str = r#"{
        "tag_name": "v0.2.0", "html_url": "https://github.com/x/y/releases/tag/v0.2.0",
        "draft": false, "prerelease": false, "immutable": true,
        "assets": [
            {"name": "SHA256SUMS", "browser_download_url": "https://e/SHA256SUMS", "digest": null},
            {"name": "opensupercad-0.2.0-x86_64-linux.tar.gz",
             "browser_download_url": "https://e/a.tar.gz", "digest": "sha256:ABC"}
        ]}"#;

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.10.0-rc.1"), Some(Version(0, 10, 0)));
        assert_eq!(Version::parse("1.2"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert_eq!(Version(1, 0, 2).to_string(), "1.0.2");
    }

    #[test]
    fn parses_release() {
        let r = Release::from_github_json(JSON).unwrap();
        assert_eq!(r.version, Version(0, 2, 0));
        assert!(r.is_newer_than(Version(0, 1, 9)));
        assert!(!r.is_newer_than(Version(0, 2, 0)));
        let a = r.asset("opensupercad-0.2.0-x86_64-linux.tar.gz").unwrap();
        assert_eq!(a.sha256.as_deref(), Some("abc"));
        let pre = JSON.replace(r#""prerelease": false"#, r#""prerelease": true"#);
        assert!(Release::from_github_json(&pre).is_err());
        // Only immutable releases, whose files can't be replaced.
        let mutable = JSON.replace(r#""immutable": true"#, r#""immutable": false"#);
        assert!(matches!(
            Release::from_github_json(&mutable),
            Err(UpdateError::NotImmutable(_))
        ));
        let unknown = JSON.replace(r#", "immutable": true"#, "");
        assert!(matches!(
            Release::from_github_json(&unknown),
            Err(UpdateError::NotImmutable(_))
        ));
    }

    #[test]
    fn checks_small_files_against_their_digest() {
        let hello = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let mut sums = Asset {
            name: "SHA256SUMS".into(),
            url: String::new(),
            sha256: Some(hello.into()),
        };
        check_digest(b"hello", &sums).unwrap();
        assert!(matches!(
            check_digest(b"hellO", &sums),
            Err(UpdateError::Checksum { .. })
        ));
        sums.sha256 = None;
        assert!(matches!(
            check_digest(b"hello", &sums),
            Err(UpdateError::NoDigest(_))
        ));
    }

    #[test]
    fn detects_install_kind() {
        let brew = Install::detect(Path::new("/opt/homebrew/Caskroom/opensupercad/1/x"));
        assert!(matches!(
            brew,
            Install::Package {
                manager: "Homebrew",
                ..
            }
        ));
        let app = Install::detect(Path::new(
            "/Applications/OpenSuperCAD.app/Contents/MacOS/opensupercad",
        ));
        assert_eq!(
            app,
            Install::MacApp {
                bundle: "/Applications/OpenSuperCAD.app".into()
            }
        );
        assert!(matches!(
            Install::detect(Path::new("/usr/bin/opensupercad")),
            Install::LinuxPackage { .. } | Install::Package { .. }
        ));
        // /usr/local is where tarballs go, not a package manager.
        assert!(!matches!(
            Install::detect(Path::new("/usr/local/bin/opensupercad")),
            Install::Package { .. }
        ));
        assert!(matches!(
            Install::detect(Path::new("/src/target/debug/opensupercad")),
            Install::Unsupported { .. }
        ));
        assert!(matches!(
            Install::detect(Path::new(r"C:\src\target\release\opensupercad.exe")),
            Install::Unsupported { .. }
        ));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Install::detect(&dir.path().join("opensupercad")),
            Install::Tarball {
                dir: dir.path().to_path_buf()
            }
        );
        // The Windows installer leaves its uninstaller next to the exe.
        std::fs::write(dir.path().join("unins000.exe"), "").unwrap();
        let installed = Install::detect(&dir.path().join("opensupercad.exe"));
        assert_eq!(
            installed,
            Install::WindowsInstaller {
                dir: dir.path().to_path_buf()
            }
        );
        assert_eq!(
            asset_name(Version(0, 7, 0), &installed, "windows", "x86_64").as_deref(),
            Some("OpenSuperCAD-0.7.0-windows-x86_64-setup.exe")
        );
        assert_eq!(
            asset_name(Version(0, 7, 0), &installed, "windows", "aarch64"),
            None
        );
    }

    #[test]
    fn picks_assets() {
        let v = Version(0, 2, 0);
        let tar = Install::Tarball { dir: "/x".into() };
        assert_eq!(
            asset_name(v, &tar, "linux", "aarch64").as_deref(),
            Some("opensupercad-0.2.0-aarch64-linux.tar.gz")
        );
        let app = Install::MacApp {
            bundle: "/A.app".into(),
        };
        assert_eq!(
            asset_name(v, &app, "macos", "aarch64").as_deref(),
            Some("OpenSuperCAD-0.2.0-macos-universal.dmg")
        );
        assert_eq!(asset_name(v, &tar, "windows", "x86_64"), None);

        let deb = Install::LinuxPackage {
            format: PackageFormat::Deb,
        };
        assert_eq!(
            asset_name(v, &deb, "linux", "aarch64").as_deref(),
            Some("opensupercad_0.2.0-1_arm64.deb")
        );
        let pacman = Install::LinuxPackage {
            format: PackageFormat::Pacman,
        };
        assert_eq!(
            asset_name(v, &pacman, "linux", "x86_64").as_deref(),
            Some("opensupercad-0.2.0-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(asset_name(v, &pacman, "linux", "aarch64"), None);
    }

    #[test]
    fn package_install_commands_are_quoted() {
        assert_eq!(
            PackageFormat::Pacman.install_command(Path::new("/home/a b/x.pkg.tar.zst")),
            "sudo pacman -U '/home/a b/x.pkg.tar.zst'"
        );
        assert_eq!(
            PackageFormat::Deb.install_command(Path::new("/tmp/it's.deb")),
            r"sudo apt install '/tmp/it'\''s.deb'"
        );
    }

    #[test]
    fn verifies_checksums() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.tar.gz");
        std::fs::write(&file, b"hello").unwrap();
        let good = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(sha256_file(&file).unwrap(), good);
        let sums = parse_sums(&format!(
            "{good}  a.tar.gz\n{}  other\nnot a line\n",
            "0".repeat(64)
        ));
        assert_eq!(sums.len(), 2);
        let mut asset = Asset {
            name: "a.tar.gz".into(),
            url: String::new(),
            sha256: Some(good.into()),
        };
        verify(&file, &asset, &sums).unwrap();
        // GitHub's digest disagrees: reject.
        asset.sha256 = Some("0".repeat(64));
        assert!(matches!(
            verify(&file, &asset, &sums),
            Err(UpdateError::Checksum { .. })
        ));
        // GitHub reports no digest: reject.
        asset.sha256 = None;
        assert!(matches!(
            verify(&file, &asset, &sums),
            Err(UpdateError::NoDigest(_))
        ));
        // Not listed in SHA256SUMS: reject.
        asset.sha256 = Some(good.into());
        asset.name = "b.tar.gz".into();
        assert!(matches!(
            verify(&file, &asset, &sums),
            Err(UpdateError::NotInSums(_))
        ));
    }

    #[test]
    fn installs_tarball_and_keeps_old() {
        let work = tempfile::tempdir().unwrap();
        let pkg = work.path().join("opensupercad-0.2.0-x86_64-linux");
        std::fs::create_dir(&pkg).unwrap();
        for b in super::BINARIES {
            std::fs::write(pkg.join(b), format!("new {b}")).unwrap();
        }
        let archive = work.path().join("a.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(work.path())
            .arg("opensupercad-0.2.0-x86_64-linux")
            .status()
            .unwrap()
            .success();
        assert!(ok);

        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("opensupercad"), "old").unwrap();
        let replaced = install_tarball(&archive, bin.path()).unwrap();
        assert_eq!(replaced.len(), 2);
        assert_eq!(
            std::fs::read_to_string(bin.path().join("opensupercad")).unwrap(),
            "new opensupercad"
        );
        assert_eq!(
            std::fs::read_to_string(bin.path().join("opensupercad.old")).unwrap(),
            "old"
        );
        // No staging folder is left behind.
        assert_eq!(std::fs::read_dir(bin.path()).unwrap().count(), 3);
    }
}
