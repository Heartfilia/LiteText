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
    time::{Duration, Instant},
};

pub(crate) const VERSION: &str = env!("LITETEXT_VERSION");
const REPOSITORY: &str = "https://github.com/Heartfilia/LiteText";
const API: &str = "https://api.github.com/repos/Heartfilia/LiteText/releases";
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    proxy_enabled: bool,
    proxy_url: String,
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
fn client(settings: &Settings, timeout: Duration) -> Result<Client> {
    let mut builder = Client::builder()
        .no_proxy()
        .https_only(true)
        .connect_timeout(Duration::from_secs(12))
        .timeout(timeout)
        .user_agent("LiteText-updater");
    if settings.proxy_enabled {
        let url = reqwest::Url::parse(settings.proxy_url.trim())?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || url.host_str().is_none()
            || url.path() != "" && url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("代理需填写 HTTP(S)/SOCKS5 代理地址，不是 GitHub 镜像前缀".into());
        }
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
    Ok(format!("LiteText-{os}-{arch}.zip"))
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

fn select_release(releases: Vec<Release>, current: &str, package: &str) -> Result<Option<Release>> {
    let include_pre = includes_prereleases(current)?;
    Ok(releases
        .into_iter()
        .filter_map(|release| {
            let parsed = version(&release.tag_name).ok()?;
            if release.draft
                || (!include_pre && (release.prerelease || !parsed.pre.is_empty()))
                || !release.assets.iter().any(|a| a.name == package)
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
    let mut releases = Vec::new();
    // The list endpoint includes prereleases and returns [] for an empty repo.
    // /latest returns 404 when only beta/rc releases exist.
    for page in 1..=10 {
        let response = http
            .get(API)
            .query(&[("per_page", 100), ("page", page)])
            .header("Accept", "application/vnd.github+json")
            .send()?
            .error_for_status()?;
        let has_next = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("rel=\"next\""));
        releases.extend(serde_json::from_slice::<Vec<Release>>(&read_limited(
            response,
            4 * 1024 * 1024,
        )?)?);
        if !has_next {
            return select_release(releases, VERSION, &asset_name()?);
        }
    }
    Err("Release list exceeds safety limit; cannot determine latest version".into())
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
fn prepare(settings: &Settings, release: &Release, progress: impl Fn(String)) -> Result<Prepared> {
    let target = installed_target(&std::env::current_exe()?.canonicalize()?)?;
    let parent = target.parent().ok_or("No installation parent")?;
    let lock_path = parent.join(".litetext-update.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|_| "Another update is in progress")?;
    // Staging in the installation directory also verifies write access, without elevation.
    let stage = tempfile::Builder::new()
        .prefix(".litetext-stage-")
        .tempdir_in(parent)?;
    let root = tempfile::Builder::new()
        .prefix("litetext-update-")
        .tempdir()?;
    let name = asset_name()?;
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
    if asset.size == 0 || asset.size > MAX_PACKAGE {
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
    let archive_path = root.path().join("package.zip");
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
        if count > asset.size {
            return Err("Download exceeds declared size".into());
        }
        output.write_all(&buffer[..len])?;
        hash.update(&buffer[..len]);
        if last.elapsed() > Duration::from_millis(200) {
            progress(format!(
                "下载 / Downloading {:.0}%",
                count as f64 / asset.size as f64 * 100.0
            ));
            last = Instant::now();
        }
    }
    output.sync_all()?;
    if count != asset.size || format!("{:x}", hash.finalize()) != expected {
        return Err("SHA-256 / size verification failed".into());
    }
    progress("校验通过，准备安装 / Verified, preparing installation".into());
    let staged = extract(&archive_path, stage.path())?;
    // Version is checked before replacement, and again by the startup acknowledgement.
    let version_file = File::create(root.path().join("package-version"))?;
    let mut process = Command::new(binary_in(&staged))
        .arg("--litetext-print-version")
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
    if let Some(ack) = ack {
        command.arg("--litetext-update-ack").arg(ack);
    }
    Ok(Some(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    ))
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
    if let Err(error) = fs::rename(&plan.target, &plan.backup) {
        let _ = launch(&plan.target, None);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&plan.staged, &plan.target) {
        fs::rename(&plan.backup, &plan.target)?;
        let _ = launch(&plan.target, None);
        return Err(error.into());
    }
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
    fs::rename(&plan.target, failed)?;
    fs::rename(&plan.backup, &plan.target)?;
    launch(&plan.target, None)?;
    Err("新版本未确认启动，已回退 / Startup not acknowledged; rolled back".into())
}

pub(crate) fn run_helper_if_requested() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == "--litetext-print-version") {
        println!("{VERSION}");
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
    fn start_update_job(&mut self, install: bool, ctx: &egui::Context) {
        if self.updater.receiver.is_some() {
            return;
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
                check(&settings).map(Event::Checked)
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
                    self.updater.message = format!("更新失败 / Update failed: {error}");
                    self.updater.receiver = None;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    pub(crate) fn update_settings_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let zh = self.lang == Lang::ZhCn;
        let busy = self.updater.receiver.is_some() || self.updater.prepared.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            ui.checkbox(
                &mut self.updates.proxy_enabled,
                if zh {
                    "使用代理访问 GitHub 更新"
                } else {
                    "Use proxy for GitHub updates"
                },
            );
            ui.add_enabled(
                self.updates.proxy_enabled,
                egui::TextEdit::singleline(&mut self.updates.proxy_url)
                    .hint_text("http://127.0.0.1:7890 / socks5h://…")
                    .desired_width(ui.available_width()),
            );
        });
        ui.label(
            egui::RichText::new(if zh {
                "仅用于检查与下载更新；关闭后直连 GitHub。"
            } else {
                "Updates only; disabled means direct GitHub access."
            })
            .small()
            .color(pal.text_faint),
        );
        ui.horizontal_wrapped(|ui| {
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
        if !self.updater.message.is_empty() {
            ui.label(egui::RichText::new(&self.updater.message).small());
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
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let response = github_icon(ui, pal);
            if response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY));
            }
            ui.label(egui::RichText::new(format!("LiteText  {VERSION}")).color(pal.text_faint));
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
            let latest = select_release(releases, current, "package.zip")
                .unwrap()
                .unwrap();
            assert_eq!(latest.tag_name, "v0.1.1-beta.10");
        }
        let releases = vec![
            release_for_test("v0.1.1-beta.10", true),
            release_for_test("v0.1.1", false),
        ];
        let latest = select_release(releases, "v0.1.1-beta.3", "package.zip")
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
        let latest = select_release(releases, "v0.1.0", "package.zip")
            .unwrap()
            .unwrap();
        assert_eq!(latest.tag_name, "v0.1.1");
        assert!(
            select_release(
                vec![release_for_test("v0.1.1-beta.3", true)],
                "v0.1.0",
                "package.zip"
            )
            .unwrap()
            .is_none()
        );
        assert!(
            select_release(vec![], "dev", "package.zip")
                .unwrap()
                .is_none()
        );
        assert!(
            select_release(
                vec![release_for_test("v0.1.1", false)],
                "dev",
                "other-platform.zip"
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
            let mut output = None;
            for _ in 0..5 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1080.0, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.settings_window(ctx, &crate::DARK),
                ));
            }
            let labels: Vec<_> = output
                .as_ref()
                .unwrap()
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
