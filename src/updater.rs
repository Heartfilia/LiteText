//! GitHub-only updater. Downloads never touch document paths. Replacement is
//! delegated to a copied executable, after the GUI exits and releases its lock.
use crate::{Lang, LiteTextApp, Palette};
use eframe::egui;
use fs2::FileExt;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) const VERSION: &str = env!("LITETEXT_VERSION");
const REPOSITORY: &str = "https://github.com/Heartfilia/LiteText";
/// Release discovery uses the repository's Atom feed instead of the REST API:
/// it is not subject to the per-IP API quota, so no authentication token is
/// ever embedded in the distributed binary.
const FEED: &str = "https://github.com/Heartfilia/LiteText/releases.atom";
const STARTUP_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const RUNNING_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    proxy_enabled: bool,
    proxy_url: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self { proxy_enabled: false, proxy_url: "http://127.0.0.1:1081".into() }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
#[derive(Clone, Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

enum Event {
    Checked(Option<Release>),
    CheckSkipped(Duration),
    Progress(String),
    Ready(Box<Prepared>),
    Failed(String),
}
#[derive(Default)]
pub(crate) struct State {
    receiver: Option<mpsc::Receiver<Event>>,
    release: Option<Release>,
    checked: Option<(Instant, Settings)>,
    message: String,
    prepared: Option<Prepared>,
    restart: bool,
    // Held until the GUI process exits; the installer waits on this exact lock.
    exit_lock: Option<File>,
    status_loaded: bool,
    last_status_check: Option<Instant>,
    next_auto_check: Option<Instant>,
    proxy_input: Option<String>,
    proxy_input_error: Option<String>,
}

impl State {
    pub(crate) fn diagnostic_status(&self) -> &str { &self.message }
}

struct Prepared {
    root: tempfile::TempDir,
    stage: tempfile::TempDir,
    plan: Plan,
    lock: File,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    target: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    lock: PathBuf,
    ack: PathBuf,
    version: String,
    status: PathBuf,
    #[serde(default)]
    runtime_lock: Option<PathBuf>,
}

fn runtime_lock_path() -> Option<PathBuf> {
    eframe::storage_dir("LiteText").map(|p| p.join("running.lock"))
}

pub(crate) fn hold_runtime_lock() -> Result<File> {
    let path = runtime_lock_path().ok_or("Application storage unavailable")?;
    fs::create_dir_all(path.parent().ok_or("Missing storage parent")?)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    FileExt::lock_shared(&lock)?;
    Ok(lock)
}

fn version(value: &str) -> Result<semver::Version> {
    Ok(semver::Version::parse(
        value.strip_prefix('v').unwrap_or(value),
    )?)
}
fn is_newer(tag: &str, current: &str) -> Result<bool> {
    let latest = version(tag)?;
    Ok(current == "dev" || latest > version(current)?)
}
fn validate_proxy(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value.trim())?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        || url.host_str().is_none() || url.path() != "" && url.path() != "/"
        || url.query().is_some() || url.fragment().is_some()
    { return Err("代理需填写 HTTP(S)/SOCKS5 代理地址，不是 GitHub 镜像前缀".into()); }
    Ok(url)
}
fn client(settings: &Settings, timeout: Duration) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .https_only(true)
        .connect_timeout(Duration::from_secs(12))
        .timeout(timeout)
        .user_agent("LiteText-updater");
    if settings.proxy_enabled {
        let url = validate_proxy(&settings.proxy_url)?;
        builder = builder.proxy(reqwest::Proxy::all(url)?);
    }
    Ok(builder.build()?)
}
fn asset_name() -> Result<String> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        _ => return Err("Unsupported OS".into()),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return Err("Unsupported CPU".into()),
    };
    Ok(package_name(os, arch))
}

fn package_name(os: &str, arch: &str) -> String {
    let extension = match os {
        "macos" => "dmg",
        "windows" => "exe",
        _ => "zip",
    };
    format!("LiteText-{os}-{arch}.{extension}")
}

fn legacy_asset_name() -> Result<String> {
    Ok(Path::new(&asset_name()?)
        .with_extension("zip")
        .to_string_lossy()
        .into_owned())
}

fn release_package(release: &Release) -> Result<String> {
    let preferred = asset_name()?;
    if release.assets.iter().any(|a| a.name == preferred) {
        return Ok(preferred);
    }
    let legacy = legacy_asset_name()?;
    if release.assets.iter().any(|a| a.name == legacy) {
        return Ok(legacy);
    }
    Err("Release asset missing for this platform".into())
}
fn validate_asset_url(asset: &Asset) -> Result<()> {
    let url = reqwest::Url::parse(&asset.browser_download_url)?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url
            .path()
            .starts_with("/Heartfilia/LiteText/releases/download/")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Unexpected GitHub release asset URL".into());
    }
    Ok(())
}
fn read_limited(mut reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Response exceeds size limit".into());
    }
    Ok(bytes)
}
fn includes_prereleases(current: &str) -> Result<bool> {
    Ok(current == "dev" || !version(current)?.pre.is_empty())
}

fn select_release(
    releases: Vec<Release>,
    current: &str,
    packages: &[&str],
) -> Result<Option<Release>> {
    let include_pre = includes_prereleases(current)?;
    Ok(releases
        .into_iter()
        .filter_map(|release| {
            let parsed = version(&release.tag_name).ok()?;
            if release.draft
                || (!include_pre && (release.prerelease || !parsed.pre.is_empty()))
                || !release
                    .assets
                    .iter()
                    .any(|a| packages.contains(&a.name.as_str()))
                || !release.assets.iter().any(|a| a.name == "SHA256SUMS")
            {
                return None;
            }
            Some((parsed, release))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, release)| release))
}

fn check(settings: &Settings) -> Result<Option<Release>> {
    let http = client(settings, Duration::from_secs(15))?;
    let preferred = asset_name()?;
    let legacy = legacy_asset_name()?;
    // The feed lists the newest releases, prereleases included, newest first.
    // Drafts are private and never appear here.
    let feed = String::from_utf8(read_limited(
        http.get(FEED).send()?.error_for_status()?,
        1024 * 1024,
    )?)?;
    let mut candidates = release_tags(&feed);
    loop {
        // Candidate tags carry the platform package and checksums until the
        // probe below decides which release actually published them.
        let releases: Vec<Release> = candidates
            .iter()
            .map(|tag| Release {
                tag_name: tag.clone(),
                draft: false,
                prerelease: version(tag)
                    .map(|parsed| !parsed.pre.is_empty())
                    .unwrap_or(false),
                assets: [preferred.clone(), "SHA256SUMS".to_owned()]
                    .into_iter()
                    .map(|name| Asset {
                        browser_download_url: download_url(tag, &name),
                        name,
                        size: 0,
                    })
                    .collect(),
            })
            .collect();
        let Some(release) = select_release(releases, VERSION, &[&preferred, &legacy])? else {
            return Ok(None);
        };
        let tag = release.tag_name;
        let mut package: Option<(String, u64)> = None;
        for name in [&preferred, &legacy] {
            if let Some(size) = probe_asset(&http, &tag, name)? {
                package = Some((name.clone(), size));
                break;
            }
        }
        let manifest = probe_asset(&http, &tag, "SHA256SUMS")?;
        if package.is_none() || manifest.is_none() {
            // A tag whose assets are still uploading (or which never shipped
            // for this platform) is skipped in favour of the previous release.
            candidates.retain(|candidate| candidate != &tag);
            continue;
        }
        let (package_name, package_size) = package.expect("checked above");
        let package_url = download_url(&tag, &package_name);
        return Ok(Some(Release {
            prerelease: release.prerelease,
            tag_name: tag.clone(),
            draft: false,
            assets: vec![
                Asset {
                    name: package_name,
                    browser_download_url: package_url,
                    size: package_size,
                },
                Asset {
                    name: "SHA256SUMS".into(),
                    browser_download_url: download_url(&tag, "SHA256SUMS"),
                    size: manifest.unwrap_or(0),
                },
            ],
        }));
    }
}

/// Extract the release tags from the Atom feed, in feed order (newest first).
/// Only the `<link href=".../releases/tag/<tag>">` target is trusted; entry
/// titles may be arbitrary release names.
fn release_tags(feed: &str) -> Vec<String> {
    const MARKER: &str = "/releases/tag/";
    let mut tags: Vec<String> = Vec::new();
    let mut rest = feed;
    while let Some(index) = rest.find(MARKER) {
        rest = &rest[index + MARKER.len()..];
        let Some(end) = rest.find('"') else { break };
        let tag = &rest[..end];
        rest = &rest[end..];
        if tag.is_empty() {
            continue;
        }
        let tag = percent_decode(tag);
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// Minimal percent-decoding for tag names; plain tags pass through unchanged.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let high = (bytes[index + 1] as char).to_digit(16).unwrap_or(0);
            let low = (bytes[index + 2] as char).to_digit(16).unwrap_or(0);
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Release asset URLs are constructed from the tag; the API list that used to
/// provide them is exactly what this change removes.
fn download_url(tag: &str, name: &str) -> String {
    format!("{REPOSITORY}/releases/download/{tag}/{name}")
}

/// Probe a constructed asset URL with HEAD. `Ok(None)` means the asset is
/// absent (release still uploading); unexpected statuses are errors.
fn probe_asset(http: &Client, tag: &str, name: &str) -> Result<Option<u64>> {
    let response = http.head(download_url(tag, name)).send()?;
    match response.status() {
        status if status.is_success() => Ok(Some(response.content_length().unwrap_or(0))),
        reqwest::StatusCode::NOT_FOUND => Ok(None),
        status => Err(format!("Unexpected GitHub response: {status}").into()),
    }
}
fn checksum(manifest: &str, name: &str) -> Result<String> {
    let entries: Vec<_> = manifest
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 2 && fields[1].trim_start_matches('*') == name).then(|| fields[0])
        })
        .collect();
    if entries.len() != 1
        || entries[0].len() != 64
        || !entries[0].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Missing, duplicate or invalid SHA256SUMS entry".into());
    }
    Ok(entries[0].to_ascii_lowercase())
}
fn installed_target(exe: &Path) -> Result<PathBuf> {
    if VERSION == "dev" {
        return Err("开发版本 dev 不会覆盖安装，请先安装 Release 版本".into());
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or("请先将 Release 的 LiteText.app 安装到可写目录")?;
        if bundle.starts_with("/Volumes") || bundle.to_string_lossy().contains("/AppTranslocation/")
        {
            return Err("请先将应用移出磁盘映像或系统隔离位置再更新".into());
        }
        Ok(bundle.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(exe.to_owned())
    }
}
fn binary_in(target: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        target.join("Contents/MacOS/litetext")
    } else {
        target.to_owned()
    }
}

fn extract(package: &Path, destination: &Path) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(File::open(package)?)?;
    if archive.len() > 4096 {
        return Err("Too many archive entries".into());
    }
    let root_name = if cfg!(target_os = "macos") {
        "LiteText.app"
    } else if cfg!(windows) {
        "litetext.exe"
    } else {
        "litetext"
    };
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let path = entry
            .enclosed_name()
            .ok_or("Unsafe archive path")?
            .to_owned();
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || path.components().next().map(|c| c.as_os_str())
                != Some(std::ffi::OsStr::new(root_name))
            || entry.name().contains('\\')
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("Unexpected archive entry or symbolic link".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Archive size overflow")?;
        if total > MAX_PACKAGE {
            return Err("Unpacked update too large".into());
        }
        let out = destination.join(path);
        if entry.is_dir() {
            fs::create_dir_all(out)?;
            continue;
        }
        fs::create_dir_all(out.parent().ok_or("No parent")?)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(&out)?;
        let count = std::io::copy(&mut entry.by_ref().take(MAX_PACKAGE + 1), &mut file)?;
        if count > MAX_PACKAGE {
            return Err("Unpacked entry too large".into());
        }
        file.sync_all()?;
    }
    let target = destination.join(root_name);
    let binary = binary_in(&target);
    if !binary.is_file() {
        return Err("Update executable is missing".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    Ok(target)
}

fn stage_package(package: &Path, name: &str, destination: &Path) -> Result<PathBuf> {
    if name.ends_with(".zip") {
        return extract(package, destination);
    }
    #[cfg(windows)]
    if name.ends_with(".exe") {
        let staged = destination.join("litetext.exe");
        fs::copy(package, &staged)?;
        return Ok(staged);
    }
    #[cfg(target_os = "macos")]
    if name.ends_with(".dmg") {
        return stage_dmg(package, destination);
    }
    Err("Unsupported update package format".into())
}

#[cfg(target_os = "macos")]
fn stage_dmg(package: &Path, destination: &Path) -> Result<PathBuf> {
    let mount = tempfile::Builder::new()
        .prefix("litetext-mount-")
        .tempdir()?;
    let status = Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(mount.path())
        .arg(package)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("Unable to mount update DMG".into());
    }
    // Detach even when validation or copying fails. The original install is
    // never touched here, and the image is always mounted read-only.
    struct Mounted<'a>(&'a Path);
    impl Drop for Mounted<'_> {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(self.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _mounted = Mounted(mount.path());
    let source = mount.path().join("LiteText.app");
    validate_bundle_tree(&source)?;
    let staged = destination.join("LiteText.app");
    let status = Command::new("/usr/bin/ditto")
        .arg(&source)
        .arg(&staged)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("Unable to stage application from DMG".into());
    }
    validate_bundle_tree(&staged)?;
    if !binary_in(&staged).is_file() {
        return Err("Update executable is missing".into());
    }
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&staged)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("Update application signature is invalid".into());
    }
    Ok(staged)
}

#[cfg(any(target_os = "macos", test))]
fn validate_bundle_tree(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0u64;
    let mut entries = 0usize;
    while let Some(path) = pending.pop() {
        entries += 1;
        if entries > 4096 {
            return Err("Too many application entries".into());
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if metadata.is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or("Application size overflow")?;
            if bytes > MAX_PACKAGE {
                return Err("Unpacked update too large".into());
            }
        } else {
            return Err(
                "Symbolic links and special files are not allowed in update application".into(),
            );
        }
    }
    Ok(())
}
/// Persistent scratch directory for updates. A portable Windows executable
/// may sit on the desktop (or any user folder); the lock and staging files
/// must not appear next to it. They live under `~/.litetext/` instead.
fn update_work_root() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(root) = work_root_override().lock().unwrap().clone() {
        return Ok(root);
    }
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    } else {
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    };
    let root = home
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or("User home directory unavailable")?
        .join(".litetext");
    fs::create_dir_all(&root)?;
    Ok(root)
}

/// Record attempts (including failed network requests), not just successes.
/// A short-lived cross-process lock prevents simultaneous windows from checking
/// on startup. No disk access happens on the GUI thread or on every frame.
#[derive(Default, Serialize, Deserialize)]
struct CheckHistory {
    last_attempt_unix: u64,
}

fn claim_update_check(root: &Path, now: u64, cooldown: Option<Duration>) -> Result<bool> {
    fs::create_dir_all(root)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(root.join("update-check.lock"))?;
    if let Err(error) = FileExt::try_lock_exclusive(&lock) {
        // Windows reports ERROR_LOCK_VIOLATION rather than Unix EWOULDBLOCK.
        // Compare fs2's platform-specific lock error instead of ErrorKind;
        // never turn a failed try-lock into an unbounded blocking wait.
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            return Ok(false);
        }
        return Err(error.into());
    }
    let path = root.join("update-check.json");
    let history = fs::read(&path).ok()
        .and_then(|bytes| serde_json::from_slice::<CheckHistory>(&bytes).ok());
    if let (Some(cooldown), Some(history)) = (cooldown, history) {
        // Treat a future timestamp as clock rollback; don't suppress forever.
        if now >= history.last_attempt_unix
            && now - history.last_attempt_unix < cooldown.as_secs()
        {
            return Ok(false);
        }
    }
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(file.as_file_mut(), &CheckHistory { last_attempt_unix: now })?;
    file.as_file_mut().sync_all()?;
    file.persist(path)?;
    Ok(true)
}

#[cfg(test)]
fn work_root_override() -> &'static std::sync::Mutex<Option<PathBuf>> {
    static OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    &OVERRIDE
}

fn prepare(settings: &Settings, release: &Release, progress: impl Fn(String)) -> Result<Prepared> {
    let target = installed_target(&std::env::current_exe()?.canonicalize()?)?;
    let work = update_work_root()?;
    let lock_path = work.join(".litetext-update.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|_| "Another update is in progress")?;
    let stage = tempfile::Builder::new()
        .prefix(".litetext-stage-")
        .tempdir_in(&work)?;
    let root = tempfile::Builder::new()
        .prefix("litetext-update-")
        .tempdir()?;
    let name = release_package(release)?;
    let find = |name: &str| -> Result<&Asset> {
        let mut matches = release.assets.iter().filter(|a| a.name == name);
        let found = matches
            .next()
            .ok_or("Release asset missing for this platform")?;
        if matches.next().is_some() {
            return Err("Duplicate release asset".into());
        }
        validate_asset_url(found)?;
        Ok(found)
    };
    let asset = find(&name)?;
    let manifest = find("SHA256SUMS")?;
    if asset.size > MAX_PACKAGE {
        return Err("Invalid release package size".into());
    }
    let http = client(settings, Duration::from_secs(900))?;
    progress("读取 SHA256SUMS / Reading checksums".into());
    let bytes = read_limited(
        http.get(&manifest.browser_download_url)
            .send()?
            .error_for_status()?,
        1024 * 1024,
    )?;
    let expected = checksum(std::str::from_utf8(&bytes)?, &name)?;
    let mut response = http
        .get(&asset.browser_download_url)
        .send()?
        .error_for_status()?;
    // The size comes from the release HEAD probe; fall back to the response
    // headers when it is unknown, and to the package cap otherwise.
    let expected_size = (asset.size > 0)
        .then_some(asset.size)
        .or_else(|| response.content_length());
    let archive_path = root.path().join(&name);
    let mut output = File::create(&archive_path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut count = 0u64;
    let mut last = Instant::now() - Duration::from_secs(1);
    loop {
        let len = response.read(&mut buffer)?;
        if len == 0 {
            break;
        }
        count += len as u64;
        if count > MAX_PACKAGE || expected_size.is_some_and(|expected| count > expected) {
            return Err("Download exceeds declared size".into());
        }
        output.write_all(&buffer[..len])?;
        hash.update(&buffer[..len]);
        if last.elapsed() > Duration::from_millis(200) {
            progress(match expected_size {
                Some(expected) => format!(
                    "下载 / Downloading {:.0}%",
                    count as f64 / expected as f64 * 100.0
                ),
                None => format!("下载 / Downloading {} MiB", count / (1024 * 1024)),
            });
            last = Instant::now();
        }
    }
    output.sync_all()?;
    if expected_size.is_some_and(|expected| count != expected)
        || format!("{:x}", hash.finalize()) != expected
    {
        return Err("SHA-256 / size verification failed".into());
    }
    progress("校验通过，准备安装 / Verified, preparing installation".into());
    drop(output);
    let staged = stage_package(&archive_path, &name, stage.path())?;
    // Version is checked before replacement, and again by the startup acknowledgement.
    let version_file = File::create(root.path().join("package-version"))?;
    let mut command = Command::new(binary_in(&staged));
    crate::windows::hide_console(&mut command);
    let mut process = command
        .arg("--litetext-print-version")
        .arg(root.path().join("package-version"))
        .stdin(Stdio::null())
        .stdout(version_file)
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = process.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = process.kill();
            let _ = process.wait();
            return Err("Package version check timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success()
        || fs::read_to_string(root.path().join("package-version"))?.trim() != release.tag_name
    {
        return Err("Package version does not match release tag".into());
    }
    let plan = Plan {
        target,
        staged,
        backup: stage.path().join("previous"),
        lock: lock_path,
        ack: root.path().join("startup-ack"),
        version: release.tag_name.clone(),
        status: status_path().ok_or("Application storage unavailable")?,
        runtime_lock: runtime_lock_path(),
    };
    Ok(Prepared {
        root,
        stage,
        plan,
        lock,
    })
}

fn launch(target: &Path, ack: Option<&Path>) -> Result<Option<std::process::Child>> {
    let mut command = Command::new(binary_in(target));
    crate::windows::hide_console(&mut command);
    if let Some(ack) = ack {
        command.arg("--litetext-update-ack").arg(ack);
    }
    crate::new_window::append_arguments(&mut command);
    Ok(Some(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    ))
}

/// Rename within a volume; fall back to a copy next to the destination when
/// the stage directory in the user profile and the installation live on
/// different volumes. Handles plain executables and `.app` bundle trees.
fn move_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Err(error) = fs::rename(source, destination) {
        // ERROR_NOT_SAME_DEVICE (Windows) / EXDEV (Unix).
        if !matches!(error.raw_os_error(), Some(17) | Some(18)) {
            return Err(error);
        }
        let incoming = destination.with_file_name(".litetext-incoming");
        let _ = remove_tree(&incoming);
        copy_tree(source, &incoming)?;
        if let Err(error) = fs::rename(&incoming, destination) {
            let _ = remove_tree(&incoming);
            return Err(error);
        }
        // Best effort: the swap already succeeded and the source may be a
        // file Windows has not fully released yet.
        let _ = remove_tree(source);
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(source)?.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, destination).map(|_| ())
    }
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(path).map(|meta| meta.is_dir()).unwrap_or(false) {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn install(plan: &Plan) -> Result<()> {
    install_with(plan, launch, Duration::from_secs(45))
}
fn install_with(
    plan: &Plan,
    launch: impl Fn(&Path, Option<&Path>) -> Result<Option<std::process::Child>>,
    startup_timeout: Duration,
) -> Result<()> {
    let lock = OpenOptions::new().read(true).write(true).open(&plan.lock)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    while lock.try_lock_exclusive().is_err() {
        if Instant::now() > deadline {
            return Err("Application did not exit; installation cancelled".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // Every GUI process holds a shared lock, including independent new windows.
    // Wait for all of them before replacing the bundle/EXE, not only the window
    // that requested the update. Otherwise Windows may reject replacement.
    let runtime_lock = plan
        .runtime_lock
        .as_ref()
        .map(|path| {
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
        })
        .transpose()?;
    if let Some(runtime_lock) = &runtime_lock {
        while runtime_lock.try_lock_exclusive().is_err() {
            if Instant::now() > deadline {
                return Err(
                    "Other LiteText windows are still running; close them and retry update".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if let Err(error) = move_path(&plan.target, &plan.backup) {
        drop(runtime_lock);
        let _ = launch(&plan.target, None);
        return Err(error.into());
    }
    if let Err(error) = move_path(&plan.staged, &plan.target) {
        move_path(&plan.backup, &plan.target)?;
        drop(runtime_lock);
        let _ = launch(&plan.target, None);
        return Err(error.into());
    }
    // New GUI needs to acquire its shared runtime lock before acknowledging.
    drop(runtime_lock);
    let mut result = launch(&plan.target, Some(&plan.ack));
    let deadline = Instant::now() + startup_timeout;
    if result.is_ok() {
        while Instant::now() < deadline {
            if fs::read_to_string(&plan.ack).is_ok_and(|v| v == plan.version) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Ok(Some(child)) = &mut result {
        // Stop the unacknowledged new process before restoring the old executable.
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        let _ = child.wait();
    }
    // Keep the failed new binary for diagnosis; never delete a running executable.
    let failed = plan.staged.with_file_name("failed-update");
    move_path(&plan.target, &failed)?;
    move_path(&plan.backup, &plan.target)?;
    launch(&plan.target, None)?;
    Err("新版本未确认启动，已回退 / Startup not acknowledged; rolled back".into())
}

pub(crate) fn run_helper_if_requested() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == "--litetext-print-version") {
        // A Windows GUI executable may not have stdout at all. An explicit
        // file also works there; older packages still print to redirected stdout.
        if let Some(path) = args.get(2) {
            let _ = fs::write(path, VERSION);
        } else {
            println!("{VERSION}");
        }
        return true;
    }
    if args.get(1).is_none_or(|s| s != "--litetext-install-update") {
        return false;
    }
    let Some(path) = args.get(2).map(PathBuf::from) else {
        return true;
    };
    let result = (|| -> Result<()> {
        let plan: Plan = serde_json::from_slice(&fs::read(&path)?)?;
        let result = install(&plan);
        let message = match &result {
            Ok(()) => format!("更新完成 / Updated to {}", plan.version),
            Err(error) => format!("更新未完成 / Update failed: {error}"),
        };
        let _ = fs::write(&plan.status, message);
        result
    })();
    if let Err(error) = result {
        let _ = fs::write(path.with_file_name("install-error.txt"), error.to_string());
    }
    true
}
fn status_path() -> Option<PathBuf> {
    crate::recovery::session_path()?
        .parent()
        .map(|p| p.join("update-result.txt"))
}
pub(crate) fn acknowledge_startup() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--litetext-update-ack")
        && let Some(path) = args.get(2).map(PathBuf::from)
        && path.file_name().is_some_and(|p| p == "startup-ack")
        && let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(path)
    {
        let _ = file.write_all(VERSION.as_bytes());
        let _ = file.sync_all();
    }
}

impl LiteTextApp {
    /// Only an Instant comparison per frame. Request one delayed repaint so an
    /// idle, continuously open app still performs its daily check.
    pub(crate) fn schedule_update_check(&mut self, ctx: &egui::Context) {
        // Workers repaint on completion and cancelling a prepared update is a
        // UI action. Don't wake an otherwise idle window while either is busy.
        if self.updater.receiver.is_some() || self.updater.prepared.is_some() {
            return;
        }
        let now = Instant::now();
        let startup = self.updater.next_auto_check.is_none();
        let deadline = *self.updater.next_auto_check.get_or_insert(now);
        if now >= deadline && self.updater.receiver.is_none() && self.updater.prepared.is_none() {
            self.start_update_job_with_cooldown(false, ctx, Some(if startup {
                STARTUP_CHECK_INTERVAL
            } else {
                RUNNING_CHECK_INTERVAL
            }));
        }
        if let Some(deadline) = self.updater.next_auto_check {
            ctx.request_repaint_after(deadline.saturating_duration_since(now).max(Duration::from_secs(1)));
        }
    }

    fn start_update_job(&mut self, install: bool, ctx: &egui::Context) {
        self.start_update_job_with_cooldown(install, ctx, None);
    }

    fn start_update_job_with_cooldown(&mut self, install: bool, ctx: &egui::Context, cooldown: Option<Duration>) {
        if self.updater.receiver.is_some() {
            return;
        }
        if !install {
            self.updater.next_auto_check = Some(Instant::now() + RUNNING_CHECK_INTERVAL);
        }
        let settings = self.updates.clone();
        let release = self.updater.release.clone();
        let (tx, rx) = mpsc::channel();
        self.updater.receiver = Some(rx);
        self.updater.message = "连接 GitHub / Connecting to GitHub…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = if install {
                release
                    .ok_or_else(|| "Check for updates first".into())
                    .and_then(|release| {
                        prepare(&settings, &release, |message| {
                            let _ = tx.send(Event::Progress(message));
                            ctx.request_repaint();
                        })
                        .map(|prepared| Event::Ready(Box::new(prepared)))
                    })
            } else {
                let now = SystemTime::now().duration_since(UNIX_EPOCH)
                    .unwrap_or_default().as_secs();
                let claimed = update_work_root()
                    .and_then(|root| claim_update_check(&root, now, cooldown));
                // Persistence failure must not disable manual or automatic checks;
                // the in-memory deadline still prevents a retry loop.
                if matches!(claimed, Ok(false)) {
                    // A different window may have checked more recently. Wake
                    // when that shared record reaches one day, not a day after
                    // this skipped attempt. Disk access stays on this worker.
                    let remaining = update_work_root().ok()
                        .and_then(|root| fs::read(root.join("update-check.json")).ok())
                        .and_then(|bytes| serde_json::from_slice::<CheckHistory>(&bytes).ok())
                        .map(|history| Duration::from_secs(
                            history.last_attempt_unix.saturating_add(RUNNING_CHECK_INTERVAL.as_secs())
                                .saturating_sub(now).clamp(60, RUNNING_CHECK_INTERVAL.as_secs())
                        ))
                        .unwrap_or(RUNNING_CHECK_INTERVAL);
                    Ok(Event::CheckSkipped(remaining))
                } else {
                    check(&settings).map(Event::Checked)
                }
            };
            let _ = tx.send(result.unwrap_or_else(|error| Event::Failed(error.to_string())));
            ctx.request_repaint();
        });
    }
    pub(crate) fn poll_updates(&mut self) {
        if !self.updater.status_loaded
            && self
                .updater
                .last_status_check
                .is_none_or(|last| last.elapsed() > Duration::from_secs(2))
        {
            self.updater.last_status_check = Some(Instant::now());
            self.read_update_result();
        }
        loop {
            let event = self.updater.receiver.as_ref().map(|rx| rx.try_recv());
            match event {
                Some(Ok(Event::CheckSkipped(remaining))) => {
                    self.updater.next_auto_check = Some(Instant::now() + remaining);
                    self.updater.receiver = None;
                    self.updater.message.clear();
                }
                Some(Ok(Event::Progress(message))) => self.updater.message = message,
                Some(Ok(Event::Checked(release))) => {
                    self.updater.message = match &release {
                        Some(release) if is_newer(&release.tag_name, VERSION).unwrap_or(false) => {
                            format!("发现新版本 / Available: {}", release.tag_name)
                        }
                        Some(_) => "已是最新版本 / Up to date".into(),
                        None if includes_prereleases(VERSION).unwrap_or(false) => {
                            "暂无适用于当前平台的已发布版本，请等待打包完成 / No compatible release available yet".into()
                        }
                        None => "暂无适用于当前平台的稳定版，仓库可能仅有测试版 / No compatible stable release; prereleases may exist".into(),
                    };
                    self.updater.release = release;
                    self.updater.checked = Some((Instant::now(), self.updates.clone()));
                    self.updater.receiver = None;
                }
                Some(Ok(Event::Ready(prepared))) => {
                    self.updater.prepared = Some(*prepared);
                    self.updater.message =
                        "更新已就绪，重启前将备份工作区 / Ready to restart".into();
                    self.updater.receiver = None;
                }
                Some(Ok(Event::Failed(error))) => {
                    self.diagnostic_event("ERROR", "updater", "Update operation failed; URL and raw error omitted");
                    self.updater.message = format!("更新失败 / Update failed: {error}");
                    self.updater.receiver = None;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.diagnostic_event("ERROR", "updater", "Update worker disconnected");
                    self.updater.message = "Update worker disconnected".into();
                    self.updater.receiver = None;
                }
                _ => break,
            }
        }
    }
    fn read_update_result(&mut self) {
        if let Some(path) = status_path()
            && let Ok(message) = fs::read_to_string(&path)
        {
            self.updater.message = message;
            let _ = fs::remove_file(path);
            self.updater.status_loaded = true;
        }
    }
    pub(crate) fn finish_update_restart(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.updater.restart {
            return;
        }
        self.updater.restart = false;
        if !self.backup_or_report() {
            return;
        }
        if let Err(error) = self.save_preferences(true) {
            self.updater.message = format!("Unable to save preferences; restart cancelled: {error}");
            return;
        }
        if let Some(storage) = frame.storage_mut() {
            eframe::set_value(storage, eframe::APP_KEY, self);
            storage.flush();
        } else {
            self.updater.message = "No settings storage; restart cancelled".into();
            return;
        }
        let Some(prepared) = self.updater.prepared.take() else {
            return;
        };
        let result = (|| -> Result<()> {
            let helper = prepared.root.path().join(if cfg!(windows) {
                "updater.exe"
            } else {
                "updater"
            });
            fs::copy(std::env::current_exe()?, &helper)?;
            let path = prepared.root.path().join("plan.json");
            fs::write(&path, serde_json::to_vec(&prepared.plan)?)?;
            let mut command = Command::new(helper);
            command
                .arg("--litetext-install-update")
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            crate::new_window::append_arguments(&mut command);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000008);
            }
            command.spawn()?;
            Ok(())
        })();
        if let Err(error) = result {
            self.updater.message = error.to_string();
            return;
        }
        // Installer owns these private directories now. Backups are deliberately
        // retained after handoff so rollback/diagnostics survive a failed restart.
        let _ = prepared.root.keep();
        let _ = prepared.stage.keep();
        self.updater.exit_lock = Some(prepared.lock);
        self.update_restarting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    pub(crate) fn update_settings_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        let busy = self.updater.receiver.is_some() || self.updater.prepared.is_some();
        if self.updates.proxy_url.trim().is_empty() { self.updates.proxy_url = Settings::default().proxy_url; }
        ui.add_enabled_ui(!busy, |ui| {
            let proxy = ui.checkbox(
                &mut self.updates.proxy_enabled,
                if zh {
                    "使用代理访问 GitHub 更新"
                } else {
                    "Use proxy for GitHub updates"
                },
            );
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("update_proxy_checkbox_rect"), proxy.rect));
            ui.add_enabled_ui(self.updates.proxy_enabled, |ui| {
                let input = self.updater.proxy_input.get_or_insert_with(|| {
                    if self.updates.proxy_url.trim().is_empty() { Settings::default().proxy_url }
                    else { self.updates.proxy_url.clone() }
                });
                let (control, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
                ui.painter().rect_filled(control, 6.0, pal.bg_elevated);
                let response = ui.put(control.shrink2(egui::vec2(50.0, 2.0)),
                    egui::TextEdit::singleline(input)
                        .font(egui::FontId::proportional(13.0))
                        .horizontal_align(egui::Align::Center).vertical_align(egui::Align::Center)
                        .frame(false).margin(egui::Margin::ZERO)
                        .hint_text("http://127.0.0.1:1081"));
                let changed = input.trim() != self.updates.proxy_url.trim();
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("update_proxy_input_rect"), response.rect);
                    d.insert_temp(egui::Id::new("update_proxy_control_rect"), control);
                    d.remove::<egui::Rect>(egui::Id::new("update_proxy_cancel_rect"));
                    d.remove::<egui::Rect>(egui::Id::new("update_proxy_confirm_rect"));
                });
                if changed {
                    for (confirm, offset) in [(false, 36.0), (true, 14.0)] {
                        let rect = egui::Rect::from_center_size(
                            egui::pos2(control.right() - offset, control.center().y), egui::vec2(20.0, 22.0));
                        let button = ui.interact(rect, egui::Id::new(("update_proxy_action", confirm)), egui::Sense::click())
                            .on_hover_text(match (confirm, zh) {
                                (true, true) => "确认修改", (false, true) => "取消修改",
                                (true, false) => "Apply", (false, false) => "Cancel",
                            }).on_hover_cursor(egui::CursorIcon::PointingHand);
                        let color = if confirm { pal.accent } else if button.hovered() { pal.danger } else { pal.text_faint };
                        if button.hovered() { ui.painter().rect_filled(rect.shrink(1.0), 4.0, color.gamma_multiply(0.12)); }
                        let c = rect.center(); let stroke = egui::Stroke::new(1.5_f32, color);
                        if confirm {
                            ui.painter().line_segment([c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.0, 3.5)], stroke);
                            ui.painter().line_segment([c + egui::vec2(-1.0, 3.5), c + egui::vec2(5.0, -3.5)], stroke);
                        } else {
                            ui.painter().line_segment([c - egui::vec2(3.5, 3.5), c + egui::vec2(3.5, 3.5)], stroke);
                            ui.painter().line_segment([c + egui::vec2(-3.5, 3.5), c + egui::vec2(3.5, -3.5)], stroke);
                        }
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(if confirm { "update_proxy_confirm_rect" } else { "update_proxy_cancel_rect" }), rect));
                        if button.clicked() {
                            if confirm {
                                let candidate = Settings { proxy_enabled: true, proxy_url: input.trim().to_owned() };
                                match validate_proxy(&candidate.proxy_url) {
                                    Ok(_) => { self.updates.proxy_url = candidate.proxy_url; *input = self.updates.proxy_url.clone(); self.updater.proxy_input_error = None; }
                                    Err(error) => self.updater.proxy_input_error = Some(error.to_string()),
                                }
                            } else {
                                *input = self.updates.proxy_url.clone(); self.updater.proxy_input_error = None;
                            }
                        }
                    }
                } else { self.updater.proxy_input_error = None; }
            });
        });
        if let Some(error) = &self.updater.proxy_input_error {
            ui.label(egui::RichText::new(error).small().color(pal.danger));
        }
        ui.label(
            egui::RichText::new(if zh {
                "仅用于检查与下载更新；关闭后直连 GitHub。"
            } else {
                "Updates only; disabled means direct GitHub access."
            })
            .small()
            .color(pal.text_faint),
        );
        let actions = ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(if zh { "检查更新" } else { "Check updates" }),
                )
                .clicked()
            {
                let cached = self
                    .updater
                    .checked
                    .as_ref()
                    .is_some_and(|(time, settings)| {
                        time.elapsed() < Duration::from_secs(120)
                            && settings.proxy_enabled == self.updates.proxy_enabled
                            && settings.proxy_url == self.updates.proxy_url
                    });
                if !cached {
                    self.updater.release = None;
                    self.start_update_job(false, ui.ctx());
                }
            }
            let available = self
                .updater
                .release
                .as_ref()
                .is_some_and(|r| is_newer(&r.tag_name, VERSION).unwrap_or(false));
            if available
                && ui
                    .add_enabled(
                        !busy && VERSION != "dev",
                        egui::Button::new(if zh {
                            "下载更新"
                        } else {
                            "Download update"
                        }),
                    )
                    .clicked()
            {
                self.start_update_job(true, ui.ctx());
            }
            if self.updater.prepared.is_some()
                && ui
                    .button(if zh {
                        "安装并重启"
                    } else {
                        "Install and restart"
                    })
                    .clicked()
            {
                self.updater.restart = true;
            }
            if self.updater.prepared.is_some()
                && ui.button(if zh { "取消" } else { "Cancel" }).clicked()
            {
                self.updater.prepared = None;
                self.updater.message = if zh {
                    "已取消安装"
                } else {
                    "Installation cancelled"
                }
                .into();
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("update_actions_rect"), actions.response.rect));
        if self.updater.prepared.is_some() {
            let hint = ui.add(egui::Label::new(egui::RichText::new(if zh {
                "安装前请先关闭其他 LiteText 窗口。"
            } else {
                "Close other LiteText windows before installing."
            }).small().color(pal.text_faint)).wrap());
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("update_install_hint_rect"), hint.rect));
        }
        if !self.updater.message.is_empty() {
            let status = ui.add(egui::Label::new(egui::RichText::new(&self.updater.message).small()).wrap());
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("update_status_rect"), status.rect));
        }
        if VERSION == "dev" {
            ui.label(
                egui::RichText::new(if zh {
                    "dev 开发构建：可检查版本，不替换本地程序。"
                } else {
                    "dev build: checks enabled, self-install disabled."
                })
                .small()
                .color(pal.text_faint),
            );
        }
    }

    /// Always-visible footer below the scrollable settings page. Keeping it
    /// outside the scroll area means adding settings never hides the version.
    pub(crate) fn settings_footer(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let response = github_icon(ui, pal);
            if response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY));
            }
            let version = ui.add(egui::Label::new(egui::RichText::new(format!("LiteText  {VERSION}")).color(pal.text_faint)).sense(egui::Sense::click()));
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("settings_version_rect"), version.rect));
            if version.clicked() {
                self.diagnostics_version_click(ui.input(|i| i.time));
            } else if ui.input(|i| i.pointer.any_click()) {
                self.diagnostics.reset_clicks();
            }
        });
    }
}

fn github_icon(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let color = if response.hovered() {
        pal.accent
    } else {
        pal.text_faint
    };
    let p = ui.painter();
    let c = rect.center();
    p.circle_filled(c, 10.0, color);
    p.circle_filled(c + egui::vec2(0.0, -0.5), 6.0, pal.bg_panel);
    for sign in [-1.0, 1.0] {
        p.add(egui::Shape::convex_polygon(
            vec![
                c + egui::vec2(sign * 5.0, -2.0),
                c + egui::vec2(sign * 5.5, -7.5),
                c + egui::vec2(sign * 1.0, -5.0),
            ],
            pal.bg_panel,
            egui::Stroke::NONE,
        ));
    }
    p.rect_filled(
        egui::Rect::from_center_size(c + egui::vec2(0.0, 7.0), egui::vec2(5.0, 7.0)),
        1.0,
        pal.bg_panel,
    );
    response
        .on_hover_text(REPOSITORY)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    #[test]
    fn skipped_check_reschedules_shared_deadline_without_an_extra_day() {
        let mut app = LiteTextApp::default();
        let (tx, rx) = mpsc::channel();
        app.updater.receiver = Some(rx);
        app.updater.status_loaded = true;
        let remaining = Duration::from_secs(2 * 60 * 60);
        tx.send(Event::CheckSkipped(remaining)).unwrap();
        let before = Instant::now();
        app.poll_updates();
        let next = app.updater.next_auto_check.unwrap();
        assert!(next >= before + remaining);
        assert!(next <= Instant::now() + remaining);
        assert!(app.updater.receiver.is_none());
    }

    #[test]
    fn scheduler_is_idle_until_deadline_and_does_not_duplicate_busy_work() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default();
        let deadline = Instant::now() + RUNNING_CHECK_INTERVAL;
        app.updater.next_auto_check = Some(deadline);
        app.schedule_update_check(&ctx);
        assert!(app.updater.receiver.is_none());
        assert_eq!(app.updater.next_auto_check, Some(deadline));
        let (_tx, rx) = mpsc::channel();
        app.updater.receiver = Some(rx);
        app.updater.next_auto_check = None;
        app.schedule_update_check(&ctx);
        assert!(app.updater.next_auto_check.is_none());
        assert!(app.updater.receiver.is_some());
    }

    #[test]
    fn proxy_input_uses_confirm_cancel_without_applying_pending_edits() {
        let ctx = egui::Context::default();
        let mut app = LiteTextApp::default(); app.updates.proxy_enabled = true;
        let draw = |app: &mut LiteTextApp, events| {
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0,500.0))),
                events, ..Default::default()
            }, |ctx| { egui::CentralPanel::default().show(ctx, |ui| app.update_settings_ui(ui, &crate::DARK)); });
        };
        draw(&mut app, vec![]);
        assert_eq!(app.updates.proxy_url, "http://127.0.0.1:1081");
        assert!(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("update_proxy_confirm_rect"))).is_none());
        for (cancel, input, expected) in [
            (true, "http://localhost:9999", "http://127.0.0.1:1081"),
            (false, "http://localhost:7890", "http://localhost:7890"),
            (false, "not a proxy", "http://localhost:7890"),
        ] {
            app.updater.proxy_input = Some(input.into()); draw(&mut app, vec![]);
            assert_ne!(app.updates.proxy_url, input);
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(if cancel { "update_proxy_cancel_rect" } else { "update_proxy_confirm_rect" }))).unwrap();
            for pressed in [true, false] { draw(&mut app, vec![egui::Event::PointerMoved(rect.center()), egui::Event::PointerButton {
                pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
            }]); }
            assert_eq!(app.updates.proxy_url, expected);
        }
        assert!(app.updater.proxy_input_error.is_some());
    }

    #[test]
    fn update_history_obeys_startup_daily_and_manual_cooldowns() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let now = 1_000_000;
        assert!(claim_update_check(root, now, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        assert!(!claim_update_check(root, now + 3599, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        assert!(claim_update_check(root, now + 3600, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        assert!(!claim_update_check(root, now + 3600 + 86399, Some(RUNNING_CHECK_INTERVAL)).unwrap());
        assert!(claim_update_check(root, now + 3600 + 86400, Some(RUNNING_CHECK_INTERVAL)).unwrap());
        // A manual request bypasses cooldown and resets the persisted timestamp.
        let manual = now + 3600 + 86401;
        assert!(claim_update_check(root, manual, None).unwrap());
        assert!(!claim_update_check(root, manual + 1, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        let history: CheckHistory = serde_json::from_slice(&fs::read(root.join("update-check.json")).unwrap()).unwrap();
        assert_eq!(history.last_attempt_unix, manual);
    }

    #[test]
    fn update_history_recovers_from_corruption_clock_rollback_and_other_windows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("update-check.json"), b"incomplete").unwrap();
        assert!(claim_update_check(root, 10000, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        assert!(claim_update_check(root, 5000, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        // Another window holding the reservation must suppress an automatic check.
        let lock = OpenOptions::new().read(true).write(true).open(root.join("update-check.lock")).unwrap();
        FileExt::lock_exclusive(&lock).unwrap();
        assert!(!claim_update_check(root, 20000, Some(STARTUP_CHECK_INTERVAL)).unwrap());
        // Manual checks must not wait forever behind another window either.
        assert!(!claim_update_check(root, 20000, None).unwrap());
        FileExt::unlock(&lock).unwrap();
        assert!(claim_update_check(root, 20000, Some(STARTUP_CHECK_INTERVAL)).unwrap());
    }

    #[test]
    fn release_feed_tags_are_extracted_newest_first_and_decoded() {
        let feed = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Recent releases</title>
  <link rel="self" href="https://github.com/Heartfilia/LiteText/releases.atom"/>
  <entry>
    <title>v0.2.0-beta.1</title>
    <link rel="alternate" type="text/html" href="https://github.com/Heartfilia/LiteText/releases/tag/v0.2.0-beta.1"/>
    <updated>2026-11-20T08:00:00Z</updated>
  </entry>
  <entry>
    <title>Release 1.2 custom name</title>
    <link rel="alternate" type="text/html" href="https://github.com/Heartfilia/LiteText/releases/tag/v0.1.4"/>
    <updated>2026-11-19T08:00:00Z</updated>
  </entry>
</feed>"#;
        assert_eq!(release_tags(feed), vec!["v0.2.0-beta.1", "v0.1.4"]);
        assert_eq!(
            release_tags(r#"<link href="https://github.com/Heartfilia/LiteText/releases/tag/v0%2E1%2E3"/>"#),
            vec!["v0.1.3"],
            "percent-encoded tag characters are decoded"
        );
        assert_eq!(release_tags("<feed></feed>"), Vec::<String>::new());
        // A truncated feed without a closing quote stops instead of panicking.
        assert_eq!(
            release_tags(r#"<link href="https://github.com/Heartfilia/LiteText/releases/tag/v0.1"#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn update_scratch_lives_in_the_user_profile_not_the_install_directory() {
        let dir = tempfile::tempdir().unwrap();
        *work_root_override().lock().unwrap() = Some(dir.path().to_owned());
        let root = update_work_root().unwrap();
        *work_root_override().lock().unwrap() = None;
        assert_eq!(root, dir.path());
        assert!(root.is_dir(), "the work root is created on demand");

        // The fallback copy used for cross-volume installs copies trees and
        // cleans up the source, so nothing is left behind.
        let source = dir.path().join("source");
        let nested = source.join("Contents/MacOS");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("litetext"), "binary").unwrap();
        fs::write(source.join("Info.plist"), "meta").unwrap();
        let destination = dir.path().join("destination");
        copy_tree(&source, &destination).unwrap();
        assert_eq!(fs::read_to_string(destination.join("Info.plist")).unwrap(), "meta");
        assert_eq!(
            fs::read_to_string(destination.join("Contents/MacOS/litetext")).unwrap(),
            "binary"
        );
        remove_tree(&source).unwrap();
        assert!(!source.exists());
        move_path(&destination, &dir.path().join("moved")).unwrap();
        assert!(dir.path().join("moved/Contents/MacOS/litetext").is_file());
    }

    #[test]
    fn package_formats_match_release_artifacts_and_legacy_zips_are_accepted() {
        assert_eq!(package_name("macos", "arm64"), "LiteText-macos-arm64.dmg");
        assert_eq!(package_name("windows", "x64"), "LiteText-windows-x64.exe");
        assert_eq!(package_name("linux", "x64"), "LiteText-linux-x64.zip");
        let preferred = asset_name().unwrap();
        let legacy = legacy_asset_name().unwrap();
        let mut release = release_for_test("v1.0.0", false);
        release.assets[0].name = legacy.clone();
        assert_eq!(release_package(&release).unwrap(), legacy);
        release.assets.push(Asset {
            name: preferred.clone(),
            browser_download_url: String::new(),
            size: 1,
        });
        assert_eq!(release_package(&release).unwrap(), preferred);
        assert!(
            select_release(vec![release], "v0.1.0", &[&preferred, &legacy])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn bundle_validation_rejects_symlinks_and_special_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("binary"), "test").unwrap();
        validate_bundle_tree(&root).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("escape")).unwrap();
            assert!(validate_bundle_tree(&root).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn raw_exe_is_staged_without_modifying_existing_install() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("package.exe");
        fs::write(&package, "MZ-test").unwrap();
        let stage = temp.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let target = stage_package(&package, "LiteText-windows-x64.exe", &stage).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"MZ-test");
        assert_eq!(fs::read(package).unwrap(), b"MZ-test");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dmg_stages_a_signed_bundle_and_detaches_image() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let app = source.join("LiteText.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::copy("/usr/bin/true", app.join("Contents/MacOS/litetext")).unwrap();
        fs::write(app.join("Contents/Info.plist"), br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleExecutable</key><string>litetext</string><key>CFBundleIdentifier</key><string>com.heartfilia.litetext.test</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#).unwrap();
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(&app)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        std::os::unix::fs::symlink("/Applications", source.join("Applications")).unwrap();
        fs::write(source.join("安装后打开失败查看.txt"), "首次安装打开说明").unwrap();
        let package = temp.path().join("package.dmg");
        assert!(
            Command::new("/usr/bin/hdiutil")
                .args(["create", "-quiet", "-srcfolder"])
                .arg(source)
                .arg(&package)
                .status()
                .unwrap()
                .success()
        );
        let stage = temp.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let staged = stage_package(&package, "LiteText-macos-arm64.dmg", &stage).unwrap();
        assert!(binary_in(&staged).is_file());
        assert!(!stage.join("Applications").exists());
        assert!(!stage.join("安装后打开失败查看.txt").exists());
    }

    #[test]
    fn update_events_stay_small_across_platforms() {
        // Prepared owns multiple platform-dependent paths and file handles.
        // Keep it indirect so Windows does not enlarge every channel message.
        assert!(std::mem::size_of::<Event>() <= 128);
    }

    #[test]
    fn boxed_ready_event_transfers_resources_and_cancel_cleans_them() {
        let root = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        let root_path = root.path().to_owned();
        let stage_path = stage.path().to_owned();
        let plan = plan_for_test(root.path());
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&plan.lock)
            .unwrap();
        let prepared = Prepared {
            root,
            stage,
            plan,
            lock,
        };
        let (tx, rx) = mpsc::channel();
        assert!(tx.send(Event::Ready(Box::new(prepared))).is_ok());
        drop(tx);
        let mut app = LiteTextApp::default();
        app.updater.status_loaded = true;
        app.updater.receiver = Some(rx);
        app.poll_updates();
        assert!(app.updater.receiver.is_none());
        assert!(app.updater.prepared.is_some());
        assert!(root_path.exists() && stage_path.exists());
        app.updater.prepared = None;
        assert!(!root_path.exists() && !stage_path.exists());
    }

    #[test]
    fn versions_compare_semantically_and_dev_never_installs() {
        assert!(is_newer("v1.10.0", "v1.9.9").unwrap());
        assert!(!is_newer("v1.2.0", "1.2.0").unwrap());
        assert!(!is_newer("v1.2.0-rc.1", "v1.2.0").unwrap());
        assert!(is_newer("v1.2.0", "dev").unwrap());
        assert!(is_newer("invalid", "dev").is_err());
        if VERSION == "dev" {
            assert!(installed_target(Path::new("/some/program")).is_err());
        }
        assert!(asset_name().unwrap().starts_with("LiteText-"));
    }

    fn release_for_test(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease,
            assets: ["package.zip", "SHA256SUMS"]
                .into_iter()
                .map(|name| Asset {
                    name: name.into(),
                    browser_download_url: String::new(),
                    size: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn beta_and_dev_channels_include_prereleases_and_stable_promotions() {
        for current in ["dev", "v0.1.1-beta.3", "v0.1.1-rc.1"] {
            let releases = vec![
                release_for_test("v0.1.1-beta.10", true),
                release_for_test("v0.1.1-beta.9", true),
            ];
            let latest = select_release(releases, current, &["package.zip"])
                .unwrap()
                .unwrap();
            assert_eq!(latest.tag_name, "v0.1.1-beta.10");
        }
        let releases = vec![
            release_for_test("v0.1.1-beta.10", true),
            release_for_test("v0.1.1", false),
        ];
        let latest = select_release(releases, "v0.1.1-beta.3", &["package.zip"])
            .unwrap()
            .unwrap();
        assert_eq!(latest.tag_name, "v0.1.1");
        assert!(is_newer(&latest.tag_name, "v0.1.1-beta.3").unwrap());
    }

    #[test]
    fn stable_channel_ignores_betas_drafts_invalid_tags_and_incomplete_packages() {
        let mut draft = release_for_test("v9.0.0", false);
        draft.draft = true;
        let mut incomplete = release_for_test("v8.0.0", false);
        incomplete.assets.pop();
        let releases = vec![
            release_for_test("v0.2.0-beta.1", true),
            release_for_test("v0.2.0-rc.1", false),
            draft,
            incomplete,
            release_for_test("not-semver", false),
            release_for_test("v0.1.1", false),
        ];
        let latest = select_release(releases, "v0.1.0", &["package.zip"])
            .unwrap()
            .unwrap();
        assert_eq!(latest.tag_name, "v0.1.1");
        assert!(
            select_release(
                vec![release_for_test("v0.1.1-beta.3", true)],
                "v0.1.0",
                &["package.zip"]
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select_release(vec![], "dev", &["package.zip"])
                .unwrap()
                .is_none()
        );
        assert!(
            select_release(
                vec![release_for_test("v0.1.1", false)],
                "dev",
                &["other-platform.zip"]
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn empty_release_check_is_not_an_http_failure_and_clears_previous_result() {
        let (tx, rx) = mpsc::channel();
        assert!(tx.send(Event::Checked(None)).is_ok());
        let mut app = LiteTextApp::default();
        app.updater.status_loaded = true;
        app.updater.release = Some(release_for_test("v0.1.1", false));
        app.updater.receiver = Some(rx);
        app.poll_updates();
        assert!(app.updater.release.is_none());
        assert!(app.updater.receiver.is_none());
        assert!(app.updater.checked.is_some());
        assert!(!app.updater.message.contains("Update failed"));
        assert!(app.updater.message.contains("暂无"));
    }

    #[test]
    fn github_proxy_settings_validate_and_persist_without_touching_terminal() {
        assert!(client(&Settings::default(), Duration::from_secs(1)).is_ok());
        for proxy_url in [
            "",
            "file:///etc/passwd",
            "http://localhost:7890/path",
            "https://host/?token=x",
        ] {
            assert!(
                client(
                    &Settings {
                        proxy_enabled: true,
                        proxy_url: proxy_url.into()
                    },
                    Duration::from_secs(1)
                )
                .is_err()
            );
        }
        for proxy_url in ["http://127.0.0.1:7890", "socks5h://127.0.0.1:1080"] {
            assert!(
                client(
                    &Settings {
                        proxy_enabled: true,
                        proxy_url: proxy_url.into()
                    },
                    Duration::from_secs(1)
                )
                .is_ok()
            );
        }
        let app: LiteTextApp = serde_json::from_str(
            r#"{"updates":{"proxy_enabled":true,"proxy_url":"http://localhost:7890"}}"#,
        )
        .unwrap();
        let restored: LiteTextApp =
            serde_json::from_str(&serde_json::to_string(&app).unwrap()).unwrap();
        assert_eq!(restored.updates.proxy_url, "http://localhost:7890");
        assert_eq!(restored.terminal, crate::terminal::Settings::default());
    }

    #[test]
    fn checksums_and_github_asset_origin_are_strict() {
        let hash = "ab".repeat(32);
        assert_eq!(
            checksum(&format!("{hash}  a.zip\n"), "a.zip").unwrap(),
            hash
        );
        for manifest in [
            "bad a.zip".to_owned(),
            format!("{hash} b.zip"),
            format!("{hash} a.zip\n{hash} a.zip"),
        ] {
            assert!(checksum(&manifest, "a.zip").is_err());
        }
        for url in [
            "http://github.com/Heartfilia/LiteText/releases/download/v1/a.zip",
            "https://evil.test/a.zip",
            "https://github.com/Other/Repo/releases/download/v1/a.zip",
        ] {
            assert!(
                validate_asset_url(&Asset {
                    name: "a.zip".into(),
                    browser_download_url: url.into(),
                    size: 1
                })
                .is_err()
            );
        }
        assert!(read_limited(&b"12345"[..], 4).is_err());
    }

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn extraction_rejects_traversal_foreign_entries_and_duplicate_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = if cfg!(target_os = "macos") {
            "LiteText.app/Contents/MacOS/litetext"
        } else if cfg!(windows) {
            "litetext.exe"
        } else {
            "litetext"
        };
        for (index, entries) in [
            vec![("../outside", b"bad".as_slice())],
            vec![("other", b"bad".as_slice())],
            vec![
                (root, b"one".as_slice()),
                (
                    if cfg!(target_os = "macos") {
                        "LiteText.app/Contents/MacOS/./litetext"
                    } else if cfg!(windows) {
                        "./litetext.exe"
                    } else {
                        "./litetext"
                    },
                    b"two".as_slice(),
                ),
            ],
        ]
        .iter()
        .enumerate()
        {
            let zip = temp.path().join(format!("bad{index}.zip"));
            archive(&zip, entries);
            assert!(extract(&zip, &temp.path().join(format!("out{index}"))).is_err());
        }
        let zip = temp.path().join("good.zip");
        archive(&zip, &[(root, b"binary")]);
        let extracted = extract(&zip, &temp.path().join("good")).unwrap();
        assert_eq!(fs::read(binary_in(&extracted)).unwrap(), b"binary");
        assert!(!temp.path().join("outside").exists());
    }

    fn plan_for_test(temp: &Path) -> Plan {
        let plan = Plan {
            target: temp.join("installed"),
            staged: temp.join("new"),
            backup: temp.join("backup"),
            lock: temp.join("lock"),
            ack: temp.join("ack"),
            version: "v1.0.1".into(),
            status: temp.join("status"),
            runtime_lock: None,
        };
        fs::write(&plan.target, "old").unwrap();
        fs::write(&plan.staged, "new").unwrap();
        fs::write(&plan.lock, "").unwrap();
        plan
    }
    #[test]
    fn transaction_installs_only_after_exit_lock_and_preserves_backup_on_success() {
        let temp = tempfile::tempdir().unwrap();
        let plan = plan_for_test(temp.path());
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&plan.lock)
            .unwrap();
        lock.lock_exclusive().unwrap();
        let target = plan.target.clone();
        let thread = std::thread::spawn(move || {
            install_with(
                &plan,
                |_, ack| {
                    fs::write(ack.unwrap(), &plan.version)?;
                    Ok(None)
                },
                Duration::from_secs(1),
            )
            .unwrap();
            plan
        });
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        drop(lock);
        let plan = thread.join().unwrap();
        assert_eq!(fs::read_to_string(plan.target).unwrap(), "new");
        assert_eq!(fs::read_to_string(plan.backup).unwrap(), "old");
    }

    #[test]
    fn transaction_waits_for_other_windows_and_releases_runtime_lock_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let mut plan = plan_for_test(temp.path());
        let path = temp.path().join("running.lock");
        let other = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        FileExt::lock_shared(&other).unwrap();
        plan.runtime_lock = Some(path.clone());
        let target = plan.target.clone();
        let thread = std::thread::spawn(move || {
            install_with(
                &plan,
                |_, ack| {
                    let gui = OpenOptions::new().read(true).write(true).open(&path)?;
                    FileExt::try_lock_shared(&gui)?;
                    fs::write(ack.unwrap(), &plan.version)?;
                    Ok(None)
                },
                Duration::from_secs(1),
            )
            .unwrap();
        });
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        drop(other);
        thread.join().unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "new");
    }
    #[test]
    fn transaction_rolls_back_failed_launch_missing_stage_and_missing_ack() {
        for mode in [0, 1, 2] {
            let temp = tempfile::tempdir().unwrap();
            let plan = plan_for_test(temp.path());
            if mode == 1 {
                fs::remove_file(&plan.staged).unwrap();
            }
            let result = install_with(
                &plan,
                |_, ack| {
                    if mode == 0 && ack.is_some() {
                        return Err("Simulated launch failure".into());
                    }
                    Ok(None)
                },
                Duration::ZERO,
            );
            assert!(result.is_err());
            assert_eq!(fs::read_to_string(plan.target).unwrap(), "old");
        }
    }

    #[test]
    fn prepared_update_rows_wrap_without_overlapping_in_settings() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            for lang in [Lang::ZhCn, Lang::En] {
                for size in [egui::vec2(640.0, 420.0), egui::vec2(1080.0, 720.0)] {
                    let ctx = egui::Context::default();
                    crate::setup_fonts(&ctx);
                    crate::apply_style(&ctx, pal);
                    let mut app = LiteTextApp { lang, show_settings: true, terminal_installed: Some(Vec::new()), ..Default::default() };
                    let root = tempfile::tempdir().unwrap();
                    let plan = plan_for_test(root.path());
                    let lock = OpenOptions::new().read(true).write(true).open(&plan.lock).unwrap();
                    app.updater.prepared = Some(Prepared { root, stage: tempfile::tempdir().unwrap(), plan, lock });
                    app.updater.release = Some(release_for_test("v999.0.0", false));
                    app.updater.message = "下载完成，SHA-256 校验通过，点击安装并重启。 Download complete; checksum verified. Your workspace will be backed up before restarting.".repeat(2);
                    let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
                    for _ in 0..5 { let _ = ctx.run(raw(), |ctx| app.settings_window(ctx, pal)); }
                    let rect = |id: &str| ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(id))).unwrap();
                    let view = rect("settings_home_scroll_rect");
                    assert!(view.top() >= 0.0 && view.bottom() <= size.y, "{size:?}: {view:?}");
                    let actions = rect("update_actions_rect");
                    let hint = rect("update_install_hint_rect");
                    let status = rect("update_status_rect");
                    assert!(actions.bottom() < hint.top(), "{actions:?} {hint:?}");
                    assert!(hint.bottom() < status.top(), "{hint:?} {status:?}");
                    for row in [actions, hint, status, rect("update_proxy_checkbox_rect")] {
                        assert!(row.left() >= view.left() && row.right() <= view.right() + 1.0, "{size:?}: {row:?} outside {view:?}");
                    }
                    // The bottom remains reachable even with the prepared-state
                    // hint and a long bilingual download-complete message.
                    for _ in 0..12 {
                        let mut input = raw();
                        input.events = vec![egui::Event::PointerMoved(view.center()), egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -1000.0), modifiers: Default::default(),
                        }];
                        let _ = ctx.run(input, |ctx| app.settings_window(ctx, pal));
                    }
                    assert!(rect("update_status_rect").bottom() <= view.bottom() + 1.0);
                }
            }
        }
    }

    #[test]
    fn settings_checkboxes_have_visible_borders_and_really_toggle() {
        for pal in [&crate::DARK, &crate::LIGHT] {
            for lang in [Lang::ZhCn, Lang::En] {
                let ctx = egui::Context::default();
                crate::setup_fonts(&ctx);
                crate::apply_style(&ctx, pal);
                let mut app = LiteTextApp { lang, show_settings: true, terminal_installed: Some(Vec::new()), ..Default::default() };
                let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 720.0))), ..Default::default() };
                for _ in 0..5 { let _ = ctx.run(raw(), |ctx| app.settings_window(ctx, pal)); }
                // The home page grew with the workspace-font entry, so the
                // terminal and update cards no longer fit one viewport:
                // scroll each region into view before asserting on it.
                let view = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_home_scroll_rect"))).unwrap();
                let wheel = |app: &mut LiteTextApp, delta: f32| {
                    let mut input = raw();
                    input.events = vec![egui::Event::PointerMoved(view.center()), egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, delta), modifiers: Default::default(),
                    }];
                    let _ = ctx.run(input, |ctx| app.settings_window(ctx, pal));
                };
                for (id, to_bottom) in [
                    ("show_terminal_toolbar_rect", false),
                    ("update_proxy_checkbox_rect", true),
                ] {
                    for _ in 0..12 {
                        wheel(&mut app, if to_bottom { -500.0 } else { 500.0 });
                    }
                    let checkbox = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(id))).unwrap();
                    let shapes = ctx.run(raw(), |ctx| app.settings_window(ctx, pal)).shapes;
                    let outline = shapes.iter().find_map(|s| match &s.shape {
                        egui::Shape::Rect(r) if checkbox.contains(r.rect.center()) && (r.rect.width() - 14.0).abs() < 1.0 => Some(r),
                        _ => None,
                    }).expect("unchecked checkbox must paint its box");
                    assert!(outline.stroke.width >= 1.0);
                    assert!((outline.rect.height() - 14.0).abs() < 1.0);
                    assert!(checkbox.height() >= 26.0, "Keep the hit area comfortable despite the smaller icon");
                    assert!(outline.stroke.color.a() > 100);
                    assert_ne!(outline.stroke.color, pal.bg_window);
                    for pressed in [true, false] {
                        let mut input = raw();
                        input.events = vec![egui::Event::PointerMoved(checkbox.center()), egui::Event::PointerButton {
                            pos: checkbox.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default(),
                        }];
                        let _ = ctx.run(input, |ctx| app.settings_window(ctx, pal));
                    }
                    // The checkmark is asserted while the card is still in view.
                    let shapes = ctx.run(raw(), |ctx| app.settings_window(ctx, pal)).shapes;
                    assert!(shapes.iter().any(|s| match &s.shape {
                        egui::Shape::Path(path) => path.stroke.width >= 1.0 && path.points.iter().all(|p| checkbox.contains(*p)),
                        _ => false,
                    }), "checked checkbox must paint a checkmark");
                }
                assert!(app.terminal.show_toolbar_button);
                assert!(app.updates.proxy_enabled);
                let refresh = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("terminal_refresh_rect"))).unwrap();
                assert!(refresh.width() < 200.0 && refresh.height() <= 28.0, "Refresh must be content-sized: {refresh:?}");
                // The scoped settings styling must not leak to toolbar buttons.
                assert_eq!(ctx.style().visuals.widgets.inactive.bg_stroke, egui::Stroke::NONE);
            }
        }
    }

    #[test]
    fn settings_footer_fits_and_has_version_instead_of_done() {
        for lang in [Lang::ZhCn, Lang::En] {
            let ctx = egui::Context::default();
            crate::setup_fonts(&ctx);
            let mut app = LiteTextApp {
                lang,
                show_settings: true,
                ..Default::default()
            };
            app.updates.proxy_enabled = true;
            app.updates.proxy_url = "http://127.0.0.1:7890".into();
            for _ in 0..5 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1080.0, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.settings_window(ctx, &crate::DARK),
                );
            }
            // The proxy row lives in the update card, below the fold of the
            // settings home scroll area since the font entry was added.
            let view = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("settings_home_scroll_rect")))
                .unwrap();
            for _ in 0..12 {
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                };
                input.events = vec![
                    egui::Event::PointerMoved(view.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -1000.0),
                        modifiers: Default::default(),
                    },
                ];
                let _ = ctx.run(input, |ctx| app.settings_window(ctx, &crate::DARK));
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1080.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.settings_window(ctx, &crate::DARK),
            );
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text()),
                    _ => None,
                })
                .collect();
            assert!(
                labels
                    .iter()
                    .any(|s| s.contains(&format!("LiteText  {VERSION}")))
            );
            assert!(!labels.contains(&crate::tr(lang).done));
            assert!(
                labels.contains(&"http://127.0.0.1:7890"),
                "Proxy must be rendered as plaintext"
            );
        }
    }

    #[test]
    fn update_restart_backup_restores_unsaved_work_without_writing_originals() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("document.txt");
        fs::write(&original, "original content").unwrap();
        let recovery = temp.path().join("session.json");
        let mut app = LiteTextApp {
            recovery_path_override: Some(recovery.clone()),
            ..Default::default()
        };
        app.open_paths(vec![original.clone()]);
        let file = app.active_mut().unwrap();
        file.draft_text = Some("unsaved edits before update".into());
        file.sync_draft();
        app.new_document();
        let file = app.active_mut().unwrap();
        file.draft_text = Some("unsaved scratch".into());
        file.sync_draft();
        assert!(app.backup_or_report());
        assert_eq!(fs::read_to_string(&original).unwrap(), "original content");
        let mut restored = LiteTextApp {
            recovery_path_override: Some(recovery),
            ..Default::default()
        };
        restored.restore_session();
        assert_eq!(restored.files.len(), 2);
        assert_eq!(
            restored.files[0].draft_text.as_deref(),
            Some("unsaved edits before update")
        );
        assert_eq!(
            restored.active().unwrap().draft_text.as_deref(),
            Some("unsaved scratch")
        );
        assert_eq!(fs::read_to_string(original).unwrap(), "original content");
    }
}
