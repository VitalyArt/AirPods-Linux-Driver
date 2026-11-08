//! Release discovery is shared; installation stays with the current package format.
use dbus::blocking::Connection;
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REPO: &str = "VitalyArt/AirPods-Linux-Driver";
const APP_ID: &str = "com.vitalyart.airpods-driver";
const MAX_PACKAGE: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Package {
    Deb,
    Rpm,
    AppImage(PathBuf),
    Flatpak { installation: String },
    Unsupported,
}

impl Package {
    pub fn detect() -> Self {
        if let Ok(info) = std::fs::read_to_string("/.flatpak-info") {
            // app-path identifies the installation which owns the running deployment.
            if !info.lines().any(|l| l == format!("name={APP_ID}")) {
                return Self::Unsupported;
            }
            let path = info.lines().find_map(|l| l.strip_prefix("app-path="));
            return match path {
                Some(p) if p.starts_with("/var/lib/flatpak/app/") => Self::Flatpak {
                    installation: "--system".into(),
                },
                Some(p) if p.contains("/.local/share/flatpak/app/") => Self::Flatpak {
                    installation: "--user".into(),
                },
                _ => Self::Unsupported,
            };
        }
        if let Some(path) = std::env::var_os("APPIMAGE") {
            let path = PathBuf::from(path);
            if path.is_absolute() && path.is_file() {
                return Self::AppImage(path);
            }
            return Self::Unsupported;
        }
        if std::env::current_exe().ok().as_deref() == Some(Path::new("/usr/bin/airpods-driver")) {
            return match std::fs::read_to_string("/usr/share/airpods-driver/package-format")
                .as_deref()
                .map(str::trim)
            {
                Ok("deb") => Self::Deb,
                Ok("rpm") => Self::Rpm,
                _ => Self::Unsupported,
            };
        }
        Self::Unsupported
    }

    fn asset_name(&self, version: &str, arch: &str) -> Option<String> {
        let deb_arch = match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            _ => return None,
        };
        Some(match self {
            Self::Deb => {
                let (base, metadata) = version.split_once('+').unwrap_or((version, ""));
                let suffix = if metadata.is_empty() {
                    String::new()
                } else {
                    format!("+{metadata}")
                };
                format!(
                    "airpods-driver_{}{suffix}_{deb_arch}.deb",
                    base.replacen('-', "~", 1)
                )
            }
            Self::Rpm => {
                let (base, metadata) = version.split_once('+').unwrap_or((version, ""));
                let release = if metadata.is_empty() {
                    "1".into()
                } else {
                    format!("1.{}", metadata.replace('-', "."))
                };
                format!(
                    "airpods-driver-{}-{release}.{arch}.rpm",
                    base.replacen('-', "~", 1)
                )
            }
            Self::AppImage(_) => format!("airpods-driver-{version}-{arch}.AppImage"),
            Self::Flatpak { .. } => format!("airpods-driver-{version}-{arch}.flatpak"),
            Self::Unsupported => return None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
    body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Available {
    pub version: String,
    pub notes: String,
    asset: Asset,
    checksum: Asset,
}

/// Owns the verified download until the user explicitly chooses Install.
#[derive(Debug)]
pub struct Prepared {
    directory: Mutex<tempfile::TempDir>,
    package: Package,
    file: PathBuf,
    digest: String,
}

#[cfg(test)]
impl Prepared {
    pub(crate) fn fixture(package: Package, content: &[u8]) -> Arc<Self> {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("update");
        std::fs::write(&file, content).unwrap();
        Arc::new(Self {
            directory: Mutex::new(directory),
            package,
            file,
            digest: hex::encode(Sha256::digest(content)),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub automatic: bool,
    pub preview: bool,
    pub last_check: u64,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            automatic: true,
            preview: !Version::parse(env!("CARGO_PKG_VERSION"))
                .unwrap()
                .pre
                .is_empty(),
            last_check: 0,
        }
    }
}
impl Preferences {
    fn path() -> PathBuf {
        crate::utils::get_app_settings_path().with_file_name("updates.json")
    }
    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file =
            tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn due(&self) -> bool {
        self.automatic && now().saturating_sub(self.last_check) >= 86400
    }
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("airpods-driver-updater")
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())
}

fn trusted_asset(asset: &Asset) -> bool {
    Url::parse(&asset.browser_download_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url
                .path()
                .starts_with(&format!("/{REPO}/releases/download/"))
    })
}

fn select(
    releases: Vec<Release>,
    package: &Package,
    current: &str,
    preview: bool,
    arch: &str,
) -> Result<Option<Available>, String> {
    let current = Version::parse(current).map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    for release in releases {
        let version = release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name);
        let Ok(parsed) = Version::parse(version) else {
            continue;
        };
        if release.draft
            || (!preview && (release.prerelease || !parsed.pre.is_empty()))
            || parsed.cmp_precedence(&current).is_le()
        {
            continue;
        }
        let Some(name) = package.asset_name(version, arch) else {
            continue;
        };
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == name && trusted_asset(a) && a.size > 0 && a.size <= MAX_PACKAGE);
        let checksum = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS" && trusted_asset(a) && a.size <= 1024 * 1024);
        if let (Some(asset), Some(checksum)) = (asset, checksum) {
            candidates.push((
                parsed,
                Available {
                    version: version.into(),
                    notes: release.body.unwrap_or_default(),
                    asset: asset.clone(),
                    checksum: checksum.clone(),
                },
            ));
        }
    }
    Ok(candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp_precedence(&b.0))
        .map(|(_, a)| a))
}

pub async fn check(package: Package, preview: bool) -> Result<Option<Available>, String> {
    if package == Package::Unsupported {
        return Err("Updates are unavailable for this installation. Use an official DEB, RPM, AppImage or Flatpak package.".into());
    }
    let client = client()?;
    let mut releases = Vec::new();
    // Bounded pagination also finds stable releases after numerous preview releases.
    for page in 1..=10 {
        let response = client
            .get(format!(
                "https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}"
            ))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if matches!(response.status().as_u16(), 403 | 429) {
            return Err("GitHub request limit reached. Try again later.".into());
        }
        let items: Vec<Release> = response
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let finished = items.len() < 100;
        releases.extend(items);
        if finished {
            break;
        }
    }
    select(
        releases,
        &package,
        env!("CARGO_PKG_VERSION"),
        preview,
        std::env::consts::ARCH,
    )
}

pub type Progress = Arc<Mutex<String>>;
fn status(progress: &Progress, value: impl Into<String>) {
    if let Ok(mut text) = progress.lock() {
        *text = value.into();
    }
}

async fn download(
    client: &Client,
    asset: &Asset,
    destination: &Path,
    progress: &Progress,
) -> Result<[u8; 32], String> {
    if !trusted_asset(asset) {
        return Err("Untrusted update address".into());
    }
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        received += chunk.len() as u64;
        if received > asset.size || received > MAX_PACKAGE {
            return Err("Update exceeds declared size".into());
        }
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        hash.update(&chunk);
        status(
            progress,
            format!("Downloading: {}%", received * 100 / asset.size.max(1)),
        );
    }
    if received != asset.size {
        return Err("Incomplete update download".into());
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(hash.finalize().into())
}

fn expected_hash(contents: &str, name: &str) -> Result<String, String> {
    let hashes: Vec<_> = contents
        .lines()
        .filter_map(|line| {
            let (hash, filename) = line.split_once(' ')?;
            (filename.trim_start().trim_start_matches('*') == name).then_some(hash)
        })
        .collect();
    if hashes.len() != 1
        || hashes[0].len() != 64
        || !hashes[0].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Missing or invalid update checksum".into());
    }
    Ok(hashes[0].to_ascii_lowercase())
}

pub async fn prepare(
    package: Package,
    update: Available,
    progress: Progress,
) -> Result<Arc<Prepared>, String> {
    // Flatpak cache is visible to the host at this same absolute path. /tmp is private.
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("librepods/updates");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let directory = tempfile::Builder::new()
        .prefix("update-")
        .tempdir_in(cache)
        .map_err(|e| e.to_string())?;
    let client = client()?;
    let checksum_path = directory.path().join("SHA256SUMS");
    download(&client, &update.checksum, &checksum_path, &progress).await?;
    let expected = expected_hash(
        &std::fs::read_to_string(checksum_path).map_err(|e| e.to_string())?,
        &update.asset.name,
    )?;
    // The server-provided filename never becomes a filesystem path.
    let extension = match &package {
        Package::Deb => "deb",
        Package::Rpm => "rpm",
        Package::AppImage(_) => "AppImage",
        Package::Flatpak { .. } => "flatpak",
        _ => return Err("Unsupported installation".into()),
    };
    let file = directory.path().join(format!("update.{extension}"));
    let digest = hex::encode(download(&client, &update.asset, &file, &progress).await?);
    if digest != expected
        || update
            .asset
            .digest
            .as_ref()
            .is_some_and(|s| s != &format!("sha256:{digest}"))
    {
        return Err("Update checksum verification failed. Nothing was installed.".into());
    }
    Ok(Arc::new(Prepared {
        directory: Mutex::new(directory),
        package,
        file,
        digest,
    }))
}

pub async fn install(prepared: Arc<Prepared>, progress: Progress) -> Result<(), String> {
    status(
        &progress,
        "Installing update… Confirm any system authorization prompt.",
    );
    tokio::task::spawn_blocking(move || {
        // Recheck the cached file after the potentially long wait for confirmation.
        let file = &prepared.file;
        let mut source = std::fs::File::open(file).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        std::io::copy(&mut source, &mut hash).map_err(|e| e.to_string())?;
        if hex::encode(hash.finalize()) != prepared.digest {
            return Err("Downloaded update changed. Download it again before installing.".into());
        }
        match &prepared.package {
            Package::Deb | Package::Rpm => {
                let result = packagekit_install(&file, &progress);
                if let Err(error) = &result {
                    if error.contains("timed out") {
                        let mut directory = prepared.directory.lock().unwrap();
                        directory.disable_cleanup(true);
                        return Err(format!(
                            "{error} Installer retained at {}",
                            directory.path().display()
                        ));
                    }
                }
                result
            }
            Package::AppImage(path) => replace_appimage(&file, &path),
            Package::Flatpak { installation } => {
                let output = Command::new("flatpak-spawn")
                    .args([
                        "--host",
                        "--watch-bus",
                        "flatpak",
                        "install",
                        &installation,
                        "--bundle",
                        "--or-update",
                        "--noninteractive",
                        "--assumeyes",
                    ])
                    .arg(&file)
                    .output()
                    .map_err(|e| format!("Cannot start the host Flatpak installer: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "Flatpak installation failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                Ok(())
            }
            Package::Unsupported => Err("Unsupported installation".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn replace_appimage(source: &Path, target: &Path) -> Result<(), String> {
    let target = std::fs::canonicalize(target).map_err(|e| e.to_string())?;
    let parent = target.parent().ok_or("Invalid AppImage path")?;
    let mut new = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Cannot write to AppImage folder: {e}"))?;
    std::io::copy(
        &mut std::fs::File::open(source).map_err(|e| e.to_string())?,
        &mut new,
    )
    .map_err(|e| e.to_string())?;
    new.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    new.as_file().sync_all().map_err(|e| e.to_string())?;
    // Back up first without moving the running image; replacement is an atomic rename.
    let mut backup_name = target.as_os_str().to_os_string();
    backup_name.push(".previous");
    let backup_path = PathBuf::from(backup_name);
    let mut backup = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    std::io::copy(
        &mut std::fs::File::open(&target).map_err(|e| e.to_string())?,
        &mut backup,
    )
    .map_err(|e| e.to_string())?;
    backup
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    backup.as_file().sync_all().map_err(|e| e.to_string())?;
    backup.persist(backup_path).map_err(|e| e.to_string())?;
    new.persist(&target).map_err(|e| e.to_string())?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn packagekit_install(file: &Path, progress: &Progress) -> Result<(), String> {
    let connection = Connection::new_system().map_err(|e| e.to_string())?;
    let manager = connection.with_proxy(
        "org.freedesktop.PackageKit",
        "/org/freedesktop/PackageKit",
        Duration::from_secs(30),
    );
    let (path,): (dbus::Path<'static>,) = manager.method_call("org.freedesktop.PackageKit", "CreateTransaction", ())
        .map_err(|e| format!("PackageKit unavailable. Install PackageKit and a PolicyKit authentication agent: {e}"))?;
    let transaction = connection.with_proxy(
        "org.freedesktop.PackageKit",
        path.clone(),
        Duration::from_secs(300),
    );
    let outcome: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
    let error = Arc::new(Mutex::new(None::<String>));
    let state = outcome.clone();
    let failure = error.clone();
    let progress = progress.clone();
    let rule = dbus::message::MatchRule::new()
        .with_type(dbus::MessageType::Signal)
        .with_sender("org.freedesktop.PackageKit")
        .with_interface("org.freedesktop.PackageKit.Transaction")
        .with_path(path);
    let token = connection
        .add_match(rule, move |_: (), _, message| {
            match message.member().as_ref().map(|m| m.as_ref()) {
                Some("ErrorCode") => {
                    if let Ok((code, details)) = message.read2::<u32, String>() {
                        *failure.lock().unwrap() =
                            Some(format!("PackageKit error {code}: {details}"));
                    }
                }
                Some("Finished") => {
                    if let Ok((exit, _)) = message.read2::<u32, u32>() {
                        let result = if exit == 1 && failure.lock().unwrap().is_none() {
                            Ok(())
                        } else {
                            Err(failure.lock().unwrap().clone().unwrap_or_else(|| {
                                format!("Package installation cancelled or failed ({exit})")
                            }))
                        };
                        *state.lock().unwrap() = Some(result);
                    }
                }
                Some("ItemProgress") => {
                    if let Ok((_, _, percent)) = message.read3::<String, u32, u32>() {
                        status(&progress, format!("Installing: {percent}%"));
                    }
                }
                _ => {}
            }
            true
        })
        .map_err(|e| e.to_string())?;
    let result = (|| {
        transaction
            .method_call::<(), _, _, _>(
                "org.freedesktop.PackageKit.Transaction",
                "SetHints",
                (vec!["interactive=true"],),
            )
            .map_err(|e| e.to_string())?;
        transaction
            .method_call::<(), _, _, _>(
                "org.freedesktop.PackageKit.Transaction",
                "InstallFiles",
                (0u64, vec![file.to_string_lossy().into_owned()]),
            )
            .map_err(|e| e.to_string())?;
        let start = Instant::now();
        loop {
            if let Some(result) = outcome.lock().unwrap().take() {
                return result;
            }
            if start.elapsed() > Duration::from_secs(1800) {
                // Do not delete an installer while PackageKit might still be reading it.
                let _: Result<(), _> =
                    transaction.method_call("org.freedesktop.PackageKit.Transaction", "Cancel", ());
                return Err(
                    "PackageKit timed out; check your system package manager before retrying."
                        .into(),
                );
            }
            connection
                .process(Duration::from_millis(200))
                .map_err(|e| e.to_string())?;
        }
    })();
    let _ = connection.remove_match(token);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn prepared_update_waits_for_install_and_keeps_download_alive() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let prepared = Prepared::fixture(Package::AppImage(target.clone()), b"new");
        let file = prepared.file.clone();
        let retained = prepared.clone();
        drop(prepared);
        assert!(file.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        install(retained, Arc::new(Mutex::new(String::new())))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn cached_file_is_rechecked_before_installation() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let prepared = Prepared::fixture(Package::AppImage(target.clone()), b"new");
        std::fs::write(&prepared.file, b"changed").unwrap();
        let error = install(prepared, Arc::new(Mutex::new(String::new())))
            .await
            .unwrap_err();
        assert!(error.contains("changed"));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn release_notes_follow_the_selected_release_and_allow_null() {
        let available = select(
            vec![release("1.0.0")],
            &Package::Deb,
            "0.1.0",
            false,
            "x86_64",
        )
        .unwrap()
        .unwrap();
        assert_eq!(available.notes, "Release notes from GitHub");
        let mut release = release("1.0.0");
        release.body = None;
        assert!(
            select(vec![release], &Package::Deb, "0.1.0", false, "x86_64")
                .unwrap()
                .unwrap()
                .notes
                .is_empty()
        );
        let release: Release = serde_json::from_str(
            r#"{"tag_name":"v1.0.0","draft":false,"prerelease":false,"assets":[],"body":null}"#,
        )
        .unwrap();
        assert!(release.body.is_none());
    }
    #[test]
    fn package_names_match_release_packaging() {
        assert_eq!(
            Package::Deb
                .asset_name("1.2.0+build-one", "aarch64")
                .unwrap(),
            "airpods-driver_1.2.0+build-one_arm64.deb"
        );
        assert_eq!(
            Package::Deb
                .asset_name("1.2.0-rc.1+build-one", "x86_64")
                .unwrap(),
            "airpods-driver_1.2.0~rc.1+build-one_amd64.deb"
        );
        assert_eq!(
            Package::Rpm
                .asset_name("1.2.0-rc.1+build-one", "aarch64")
                .unwrap(),
            "airpods-driver-1.2.0~rc.1-1.build.one.aarch64.rpm"
        );
        assert_eq!(
            Package::AppImage("/app".into())
                .asset_name("1.2.0", "x86_64")
                .unwrap(),
            "airpods-driver-1.2.0-x86_64.AppImage"
        );
        assert_eq!(
            Package::Flatpak {
                installation: "--user".into()
            }
            .asset_name("1.2.0", "aarch64")
            .unwrap(),
            "airpods-driver-1.2.0-aarch64.flatpak"
        );
        assert!(Package::Deb.asset_name("1.2.0", "i686").is_none());
    }
    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: format!(
                "https://github.com/{REPO}/releases/download/v1.0.0/{name}"
            ),
            size: 10,
            digest: None,
        }
    }
    fn release(version: &str) -> Release {
        Release {
            tag_name: format!("v{version}"),
            draft: false,
            prerelease: false,
            assets: vec![
                asset(&format!("airpods-driver_{version}_amd64.deb")),
                asset("SHA256SUMS"),
            ],
            body: Some("Release notes from GitHub".into()),
        }
    }
    #[test]
    fn selection_respects_channel_format_architecture_and_version() {
        let releases = vec![release("1.0.0"), release("1.1.0"), release("1.2.0-beta.1")];
        assert_eq!(
            select(releases.clone(), &Package::Deb, "1.0.0", false, "x86_64")
                .unwrap()
                .unwrap()
                .version,
            "1.1.0"
        );
        assert!(
            select(releases.clone(), &Package::Deb, "1.1.0", false, "x86_64")
                .unwrap()
                .is_none()
        );
        assert!(
            select(releases.clone(), &Package::Rpm, "1.0.0", true, "x86_64")
                .unwrap()
                .is_none()
        );
        assert!(
            select(releases, &Package::Deb, "1.0.0", true, "aarch64")
                .unwrap()
                .is_none()
        );
        let mut beta = release("1.2.0-beta.1");
        beta.assets[0].name = "airpods-driver_1.2.0~beta.1_amd64.deb".into();
        assert!(
            select(vec![beta.clone()], &Package::Deb, "1.1.0", false, "x86_64")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            select(vec![beta], &Package::Deb, "1.1.0", true, "x86_64")
                .unwrap()
                .unwrap()
                .version,
            "1.2.0-beta.1"
        );
    }
    #[test]
    fn untrusted_or_incomplete_release_is_rejected() {
        let mut r = release("2.0.0");
        r.assets[0].browser_download_url = "https://example.org/update.deb".into();
        assert!(
            select(vec![r], &Package::Deb, "1.0.0", false, "x86_64")
                .unwrap()
                .is_none()
        );
        let mut r = release("2.0.0");
        r.assets.pop();
        assert!(
            select(vec![r], &Package::Deb, "1.0.0", false, "x86_64")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn checksum_requires_exact_unique_filename() {
        let hash = "ab".repeat(32);
        assert_eq!(
            expected_hash(&format!("{hash}  app.deb\n"), "app.deb").unwrap(),
            hash
        );
        assert!(expected_hash(&format!("{hash}  other.deb\n"), "app.deb").is_err());
        assert!(expected_hash("abc  app.deb", "app.deb").is_err());
        assert!(expected_hash(&format!("{hash}  app.deb\n{hash}  app.deb"), "app.deb").is_err());
    }
    #[test]
    fn appimage_replacement_preserves_backup_and_executable_mode() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("app.AppImage");
        let new = dir.path().join("download");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        replace_appimage(&new, &old).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), b"new");
        assert_eq!(
            std::fs::read(dir.path().join("app.AppImage.previous")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::metadata(&old).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(replace_appimage(&dir.path().join("missing"), &old).is_err());
        assert_eq!(std::fs::read(&old).unwrap(), b"new");
    }
}
