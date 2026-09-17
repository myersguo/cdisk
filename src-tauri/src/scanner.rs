use crate::models::*;
use crate::safety;
use crate::task::ScanControl;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const DATA_VOLUME: &str = "/System/Volumes/Data";
const RECENT_SECONDS: u64 = 7 * 24 * 3600;
const OLD_LOG_SECONDS: u64 = 30 * 24 * 3600;
const STALE_SECONDS: u64 = 7 * 24 * 3600;
pub const MAX_RESULTS: usize = 500;

fn retain_largest(targets: &mut Vec<Target>, target: Target) -> bool {
    targets.push(target);
    if targets.len() <= MAX_RESULTS {
        return false;
    }
    targets.sort_by(|a, b| b.item.bytes.cmp(&a.item.bytes));
    targets.pop();
    true
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn home() -> Result<PathBuf, String> {
    let raw = std::env::var_os("HOME").ok_or("无法读取用户目录")?;
    fs::canonicalize(raw).map_err(|e| e.to_string())
}

pub fn disk() -> Result<DiskStats, String> {
    let path = std::ffi::CString::new(DATA_VOLUME).map_err(|error| error.to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: static NUL-terminated path; statfs initializes stats on success.
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let stats = unsafe { stats.assume_init() };
    Ok(DiskStats {
        total_bytes: stats.f_blocks.saturating_mul(u64::from(stats.f_bsize)),
        available_bytes: stats.f_bavail.saturating_mul(u64::from(stats.f_bsize)),
    })
}

#[derive(Default, Debug)]
struct Measurement {
    bytes: u64,
    entries: u64,
    latest: Option<u64>,
    complete: bool,
    sensitive: bool,
}

struct Walk<'a, F: FnMut(ScanProgress), C: FnMut(Target)> {
    id: &'a str,
    mode: ScanMode,
    cancel: &'a ScanControl,
    emit: F,
    record_target: C,
    mounts: Vec<PathBuf>,
    settings: &'a Settings,
    entries: u64,
    errors: u64,
    skipped: u64,
    last_emit: Instant,
    next_id: u64,
}

impl<F: FnMut(ScanProgress), C: FnMut(Target)> Walk<'_, F, C> {
    fn cancelled(&self) -> bool {
        !self.cancel.checkpoint()
    }

    fn progress(&mut self, path: &Path, force: bool) {
        if force || self.last_emit.elapsed().as_millis() >= 120 {
            (self.emit)(ScanProgress {
                scan_id: self.id.into(),
                mode: self.mode,
                scanned_entries: self.entries,
                unreadable_entries: self.errors,
                current_path: path.display().to_string(),
                candidate: None,
            });
            self.last_emit = Instant::now();
        }
    }

    fn record(&mut self, targets: &mut Vec<Target>, mut target: Target) -> bool {
        target.item.id = format!("item-{}", self.next_id);
        self.next_id += 1;
        (self.record_target)(target.clone());
        (self.emit)(ScanProgress {
            scan_id: self.id.into(),
            mode: self.mode,
            scanned_entries: self.entries,
            unreadable_entries: self.errors,
            current_path: target.item.path.clone(),
            candidate: Some(target.item.clone()),
        });
        retain_largest(targets, target)
    }

    fn skip(&self, path: &Path, root: &Path) -> bool {
        (path != root && self.mounts.iter().any(|p| p == path))
            || safety::excluded(path, self.settings)
            || path == Path::new("/System/Volumes/Data/Volumes")
    }

    fn measure(&mut self, root: &Path) -> Measurement {
        let mut result = Measurement {
            complete: true,
            ..Default::default()
        };
        let root_device = match fs::symlink_metadata(root) {
            Ok(meta) => meta.dev(),
            Err(_) => {
                self.errors += 1;
                result.complete = false;
                return result;
            }
        };
        let mut hardlinks = HashSet::new();
        self.visit(root, root, root_device, 0, &mut hardlinks, &mut result);
        result
    }

    fn visit(
        &mut self,
        path: &Path,
        root: &Path,
        device: u64,
        depth: usize,
        hardlinks: &mut HashSet<(u64, u64)>,
        result: &mut Measurement,
    ) {
        if self.cancelled() {
            result.complete = false;
            return;
        }
        // ponytail: 128 levels prevents recursive stack exhaustion; deeper scans need an iterative walker.
        if depth >= 128 || self.skip(path, root) {
            self.skipped += 1;
            result.complete = false;
            return;
        }
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(_) => {
                self.errors += 1;
                result.complete = false;
                return;
            }
        };
        if path.file_name().is_some_and(|n| n == ".git") {
            result.sensitive = true;
        }
        if meta.file_type().is_symlink() || meta.dev() != device {
            self.skipped += 1;
            if meta.dev() != device {
                result.complete = false;
            }
            return;
        }
        self.entries += 1;
        result.entries += 1;
        self.progress(path, false);
        let modified = u64::try_from(meta.mtime()).ok();
        result.latest = result.latest.max(modified);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if matches!(
            name.as_str(),
            ".git" | ".ssh" | ".aws" | ".gnupg" | ".env" | "id_rsa" | "id_ed25519"
        ) || name.starts_with(".env.")
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.ends_with(".p12")
            || name.contains("keypair")
        {
            result.sensitive = true;
        }
        if !meta.is_dir() {
            if !meta.is_file() {
                return;
            }
            if meta.nlink() > 1 {
                // ponytail: abort cleanup eligibility at 100k hard-linked inodes; use disk-backed accounting beyond this.
                if hardlinks.len() >= 100_000 {
                    result.complete = false;
                    self.skipped += 1;
                    return;
                }
                if !hardlinks.insert((meta.dev(), meta.ino())) {
                    return;
                }
            }
            result.bytes = result
                .bytes
                .saturating_add(meta.blocks().saturating_mul(512));
            return;
        }
        result.bytes = result
            .bytes
            .saturating_add(meta.blocks().saturating_mul(512));
        let children = match fs::read_dir(path) {
            Ok(children) => children,
            Err(_) => {
                self.errors += 1;
                result.complete = false;
                return;
            }
        };
        for child in children {
            if self.cancelled() {
                result.complete = false;
                break;
            }
            match child {
                Ok(child) => self.visit(&child.path(), root, device, depth + 1, hardlinks, result),
                Err(_) => {
                    self.errors += 1;
                    result.complete = false;
                }
            }
        }
    }

    fn target(
        &mut self,
        path: &Path,
        title: &str,
        category: &str,
        patterns: &[&str],
        home: &Path,
        processes: &Result<String, String>,
    ) -> Option<Target> {
        if !path.try_exists().unwrap_or(true) {
            return None;
        }
        self.progress(path, true);
        let identity = safety::capture(path);
        let measurement = if identity.is_ok() {
            self.measure(path)
        } else {
            Measurement {
                complete: false,
                ..Default::default()
            }
        };
        let owner_patterns = patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let excluded_by = self
            .settings
            .excluded_paths
            .iter()
            .find(|p| path.starts_with(p))
            .cloned();
        let mut failure = identity.as_ref().err().map(|e| ("system", e.clone()));
        if failure.is_none() {
            failure = safety::protect(path, home, self.settings).err().map(|e| {
                (
                    if excluded_by.is_some() {
                        "manual"
                    } else {
                        "system"
                    },
                    e,
                )
            });
        }
        if failure.is_none() && !measurement.complete {
            failure = Some((
                "incomplete",
                "此项尚未完整扫描，或包含不可读/挂载/保护项，请重新检查此项".into(),
            ));
        }
        if failure.is_none() && measurement.sensitive {
            failure = Some(("system", "发现嵌套 Git、环境配置或密钥文件".into()));
        }
        if failure.is_none() {
            failure = daily_policy(path, category, home)
                .err()
                .map(|error| ("system", error));
        }
        if category == "项目产物" && failure.is_none() {
            failure = safety::project_proof(path).err().map(|e| ("system", e));
        }
        if category == "安装包" && failure.is_none() {
            failure = match safety::installer_is_mounted(path) {
                Ok(false) => None,
                Ok(true) => Some((
                    "mounted",
                    "安装镜像仍已挂载；请在 Finder 推出镜像，再点击“重新检查此项”".into(),
                )),
                Err(error) => Some(("unknown", error)),
            };
        }
        if failure.is_none() {
            failure = safety::usage_check(path, &owner_patterns, processes)
                .err()
                .map(|block| (block.protection, block.reason));
        }
        let recent = measurement
            .latest
            .is_none_or(|time| now().saturating_sub(time) < RECENT_SECONDS);
        let cleanable = failure.is_none() && measurement.complete;
        let protection = failure.as_ref().map(|(kind, _)| kind.to_string());
        let (status, reason) = if let Some((kind, reason)) = failure {
            (
                if kind == "incomplete" {
                    "partial"
                } else {
                    kind
                },
                reason,
            )
        } else if recent {
            (
                "review",
                "最近 7 天有文件活动，不默认选择；清理后需要重建/下载".into(),
            )
        } else {
            ("ready", "可重新生成；仍需确认后才会永久删除".into())
        };
        Some(Target {
            item: Candidate {
                id: String::new(),
                title: title.into(),
                path: path.display().to_string(),
                category: category.into(),
                description: match category {
                    "项目产物" => {
                        "只清理 Git ignore 的生成目录；保留跟踪文件、嵌套仓库和敏感配置。"
                    }
                    "安装包" => "先确认安装完成且不再需要；已挂载或运行中的安装包不能清理。",
                    "系统缓存" => {
                        "只清理当前用户目录内的 macOS 缓存与诊断数据；不访问系统级缓存。"
                    }
                    "浏览器数据" => {
                        "只清理浏览器缓存与临时网页数据；保留 Cookie、历史、书签、密码和登录会话。"
                    }
                    "日志与诊断" => "只清理 30 天以上的明确崩溃报告和日志。",
                    "过期临时文件" => {
                        "只清理至少 7 天未使用的明确临时文件；恢复草稿始终受保护。"
                    }
                    "下载残留" => "只清理至少 7 天未使用的未完成下载残留。",
                    "废纸篓" => "废纸篓项目始终需要手动选择，永久删除后无法恢复。",
                    _ => "只清理指定缓存或日志目录，不清理账号、会话和系统数据。",
                }
                .into(),
                bytes: measurement.bytes,
                entries: measurement.entries,
                modified_at: measurement.latest,
                is_dir: fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()),
                cleanable,
                recommended: cleanable && !recent,
                status: status.into(),
                reason,
                protection,
                excluded_by,
                complete: measurement.complete,
            },
            identity: identity.ok(),
            owner_patterns,
        })
    }

    fn discover<D: FnMut(&mut Self, PathBuf)>(
        &mut self,
        root: &Path,
        depth: usize,
        projects: bool,
        found: &mut HashSet<PathBuf>,
        on_found: &mut D,
    ) {
        if self.cancelled() {
            return;
        }
        if self.skip(root, Path::new("")) {
            self.skipped += 1;
            return;
        }
        let meta = match fs::symlink_metadata(root) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => meta,
            _ => return,
        };
        self.entries += 1;
        self.progress(root, false);
        if depth >= 8 {
            self.skipped += 1;
            return;
        }
        let Ok(entries) = fs::read_dir(root) else {
            self.errors += 1;
            return;
        };
        for entry in entries {
            if self.cancelled() {
                break;
            }
            // ponytail: at most 10k discovered artifacts per pass; add a nearer project root beyond this.
            if found.len() >= 10_000 {
                self.skipped += 1;
                break;
            }
            let Ok(entry) = entry else {
                self.errors += 1;
                continue;
            };
            let path = entry.path();
            let Ok(child) = fs::symlink_metadata(&path) else {
                self.errors += 1;
                continue;
            };
            if child.file_type().is_symlink() || child.dev() != meta.dev() {
                self.skipped += 1;
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name.starts_with(".cdisk-") {
                continue;
            }
            if projects
                && matches!(
                    name.as_ref(),
                    "target"
                        | "node_modules"
                        | ".venv"
                        | ".next"
                        | ".build"
                        | "build"
                        | "dist"
                        | "output"
                )
            {
                if child.is_dir() && found.insert(path.clone()) {
                    on_found(self, path);
                }
            } else if !projects && child.is_file() && is_installer(&path) {
                if found.insert(path.clone()) {
                    on_found(self, path);
                }
            } else if child.is_dir() && !name.ends_with(".app") {
                self.discover(&path, depth + 1, projects, found, on_found);
            }
        }
    }
}

pub fn is_installer(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
        matches!(
            x.to_lowercase().as_str(),
            "dmg" | "pkg" | "mpkg" | "xip" | "iso"
        )
    })
}

pub fn default_roots(home: &Path) -> Vec<String> {
    [
        "Projects",
        "Developer",
        "repos",
        "go/src",
        "agent_ctx",
        "work",
    ]
    .iter()
    .map(|p| home.join(p))
    .filter(|p| p.is_dir())
    .map(|p| p.display().to_string())
    .collect()
}

struct DailyRule {
    relative: &'static str,
    title: &'static str,
    category: DailyCategory,
    owners: &'static [&'static str],
}

// Every rule stays under the current user's home directory. "System cache"
// means user-scoped macOS diagnostic/temp data, never /System or /Library.
// Browser rules intentionally exclude history, cookies, passwords and sessions.
const RULES: &[DailyRule] = &[
    DailyRule {
        relative: "Library/Caches/com.apple.helpd",
        title: "macOS 帮助缓存",
        category: DailyCategory::System,
        owners: &[],
    },
    DailyRule {
        relative: "Library/Caches/com.apple.nsurlsessiond",
        title: "用户网络缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".go/build-cache",
        title: "Go 编译缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "Library/Caches/go-build",
        title: "Go 默认编译缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".go/cache",
        title: "Go 模块下载",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "go/pkg/mod",
        title: "Go 默认模块缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".npm-user-cache/_cacache",
        title: "npm 下载缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".npm/_cacache",
        title: "npm 默认缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "Library/pnpm/store",
        title: "pnpm 共享缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".bun/install/cache",
        title: "Bun 下载缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: ".cache/uv",
        title: "uv Python 缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "Library/Caches/pip",
        title: "pip 下载缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "Library/Caches/Homebrew",
        title: "Homebrew 下载缓存",
        category: DailyCategory::User,
        owners: &[],
    },
    DailyRule {
        relative: "Library/Caches/com.microsoft.VSCode.ShipIt",
        title: "VS Code 更新包",
        category: DailyCategory::Application,
        owners: &["Visual Studio Code.app"],
    },
    DailyRule {
        relative: "Library/Caches/LarkShell",
        title: "飞书缓存",
        category: DailyCategory::Application,
        owners: &["Lark.app", "Feishu.app"],
    },
    DailyRule {
        relative: "Library/Caches/com.tencent.xinWeChat",
        title: "微信缓存",
        category: DailyCategory::Application,
        owners: &["WeChat.app"],
    },
    DailyRule {
        relative: "Library/Caches/Google/Chrome",
        title: "Chrome 网页缓存",
        category: DailyCategory::Browser,
        owners: &["Google Chrome.app"],
    },
    DailyRule {
        relative: "Library/Caches/Firefox/Profiles",
        title: "Firefox 网页缓存",
        category: DailyCategory::Browser,
        owners: &["Firefox.app"],
    },
    DailyRule {
        relative: "Library/Caches/Microsoft Edge",
        title: "Edge 网页缓存",
        category: DailyCategory::Browser,
        owners: &["Microsoft Edge.app"],
    },
    DailyRule {
        relative: "Library/Caches/BraveSoftware",
        title: "Brave 网页缓存",
        category: DailyCategory::Browser,
        owners: &["Brave Browser.app"],
    },
    DailyRule {
        relative: "Library/Caches/company.thebrowser.Browser",
        title: "Arc 网页缓存",
        category: DailyCategory::Browser,
        owners: &["Arc.app"],
    },
];

fn daily_category(category: DailyCategory) -> &'static str {
    match category {
        DailyCategory::System => "系统缓存",
        DailyCategory::User => "用户缓存",
        DailyCategory::Application => "应用缓存",
        DailyCategory::Browser => "浏览器数据",
        DailyCategory::Logs => "日志与诊断",
        DailyCategory::Temporary => "过期临时文件",
        DailyCategory::Downloads => "下载残留",
        DailyCategory::Trash => "废纸篓",
    }
}

fn older_than(meta: &fs::Metadata, seconds: u64) -> bool {
    u64::try_from(meta.mtime())
        .ok()
        .is_some_and(|modified| now().saturating_sub(modified) >= seconds)
}

fn stale_download(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "crdownload" | "download" | "partial" | "part"
            )
        })
}

fn temporary_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    name.ends_with(".tmp") || name.ends_with(".temp") || name.ends_with(".partial")
}

fn trash_volumes() -> Vec<PathBuf> {
    safety::mount_paths()
        .unwrap_or_default()
        .into_iter()
        .filter(|mount| mount.starts_with("/Volumes"))
        .filter_map(|mount| {
            let uid = unsafe { libc::geteuid() };
            let path = mount.join(".Trashes").join(uid.to_string());
            path.is_dir().then_some(path)
        })
        .collect()
}

struct DailySource {
    root: PathBuf,
    title: &'static str,
    trash: bool,
    depth: usize,
    owners: &'static [&'static str],
}

fn daily_sources(category: DailyCategory, home: &Path) -> Vec<DailySource> {
    match category {
        DailyCategory::Logs => vec![
            DailySource {
                root: home.join("Library/Logs/DiagnosticReports"),
                title: "诊断报告",
                trash: false,
                depth: 1,
                owners: &[],
            },
            DailySource {
                root: home.join("Library/Logs/com.openai.codex"),
                title: "Codex 旧日志",
                trash: false,
                depth: 2,
                owners: &["Codex.app", "ChatGPT.app", "codex"],
            },
            DailySource {
                root: home.join("Library/Logs/JetBrains"),
                title: "JetBrains 旧日志",
                trash: false,
                depth: 3,
                owners: &["GoLand.app", "PyCharm.app", "IntelliJ IDEA.app"],
            },
            DailySource {
                root: home.join("Library/Logs"),
                title: "旧日志",
                trash: false,
                depth: 1,
                owners: &[],
            },
        ],
        DailyCategory::Temporary => fs::canonicalize(std::env::temp_dir())
            .ok()
            .map(|root| {
                vec![DailySource {
                    root,
                    title: "过期临时文件",
                    trash: false,
                    depth: 1,
                    owners: &[],
                }]
            })
            .unwrap_or_default(),
        DailyCategory::Downloads => vec![
            DailySource {
                root: home.join("Downloads"),
                title: "失败下载残留",
                trash: false,
                depth: 1,
                owners: &[],
            },
            DailySource {
                root: home.join("Desktop"),
                title: "失败下载残留",
                trash: false,
                depth: 1,
                owners: &[],
            },
        ],
        DailyCategory::Trash => {
            let mut sources = vec![DailySource {
                root: home.join(".Trash"),
                title: "废纸篓项目",
                trash: true,
                depth: 1,
                owners: &[],
            }];
            sources.extend(trash_volumes().into_iter().map(|root| DailySource {
                root,
                title: "外置磁盘废纸篓项目",
                trash: true,
                depth: 1,
                owners: &[],
            }));
            sources
        }
        _ => vec![],
    }
}

fn daily_source_matches(
    category: DailyCategory,
    path: &Path,
    metadata: &fs::Metadata,
    trash: bool,
) -> bool {
    if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
        return false;
    }
    match category {
        DailyCategory::Logs => {
            metadata.is_file()
                && older_than(metadata, OLD_LOG_SECONDS)
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "log" | "crash" | "diag" | "ips"
                        )
                    })
        }
        DailyCategory::Temporary => {
            metadata.is_file() && older_than(metadata, STALE_SECONDS) && temporary_name(path)
        }
        DailyCategory::Downloads => {
            metadata.is_file() && older_than(metadata, STALE_SECONDS) && stale_download(path)
        }
        DailyCategory::Trash => trash,
        _ => false,
    }
}

fn is_daily_hygiene_category(category: &str) -> bool {
    matches!(
        category,
        "日志与诊断" | "过期临时文件" | "下载残留" | "废纸篓"
    )
}

fn valid_trash_item(path: &Path, home: &Path) -> bool {
    if path.parent() == Some(home.join(".Trash").as_path()) {
        return true;
    }
    let uid = unsafe { libc::geteuid() }.to_string();
    path.parent().is_some_and(|parent| {
        parent.starts_with("/Volumes")
            && parent.file_name().is_some_and(|name| name == uid.as_str())
            && parent
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".Trashes")
    })
}

fn daily_policy(path: &Path, category: &str, home: &Path) -> Result<(), String> {
    if !is_daily_hygiene_category(category) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let allowed = match category {
        "日志与诊断" => {
            let roots = [
                home.join("Library/Logs/DiagnosticReports"),
                home.join("Library/Logs/com.openai.codex"),
                home.join("Library/Logs/JetBrains"),
                home.join("Library/Logs"),
            ];
            roots.iter().any(|root| path.starts_with(root))
                && daily_source_matches(DailyCategory::Logs, path, &metadata, false)
        }
        "过期临时文件" => {
            let root = fs::canonicalize(std::env::temp_dir()).map_err(|error| error.to_string())?;
            path.starts_with(root)
                && daily_source_matches(DailyCategory::Temporary, path, &metadata, false)
        }
        "下载残留" => {
            matches!(path.parent(), Some(parent) if parent == home.join("Downloads") || parent == home.join("Desktop"))
                && daily_source_matches(DailyCategory::Downloads, path, &metadata, false)
        }
        "废纸篓" => {
            valid_trash_item(path, home)
                && daily_source_matches(DailyCategory::Trash, path, &metadata, true)
        }
        _ => false,
    };
    allowed
        .then_some(())
        .ok_or_else(|| "项目不再符合日常清理规则，请重新扫描".into())
}

fn scan_daily_source<F: FnMut(ScanProgress), C: FnMut(Target)>(
    walk: &mut Walk<'_, F, C>,
    category: DailyCategory,
    home: &Path,
    processes: &Result<String, String>,
    targets: &mut Vec<Target>,
) -> bool {
    let mut truncated = false;
    for source in daily_sources(category, home) {
        let mut directories = vec![(source.root, 0_usize)];
        let mut visited = 0_usize;
        while let Some((directory, depth)) = directories.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries {
                visited += 1;
                if visited > 10_000 {
                    walk.skipped += 1;
                    break;
                }
                if walk.cancelled() {
                    return truncated;
                }
                let Ok(entry) = entry else {
                    walk.errors += 1;
                    continue;
                };
                let path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    walk.errors += 1;
                    continue;
                };
                if category == DailyCategory::Logs
                    && metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && depth + 1 < source.depth
                {
                    directories.push((path, depth + 1));
                    continue;
                }
                if !daily_source_matches(category, &path, &metadata, source.trash) {
                    continue;
                }
                let item_title = if category == DailyCategory::Trash {
                    entry.file_name().to_string_lossy().into_owned()
                } else {
                    format!("{} · {}", source.title, entry.file_name().to_string_lossy())
                };
                if let Some(mut target) = walk.target(
                    &path,
                    &item_title,
                    daily_category(category),
                    source.owners,
                    home,
                    processes,
                ) {
                    if category == DailyCategory::Trash {
                        target.item.recommended = false;
                        if target.item.cleanable {
                            target.item.status = "review".into();
                            target.item.reason = "废纸篓项目需手动选择；永久删除后无法恢复".into();
                        }
                    }
                    truncated |= walk.record(targets, target);
                }
            }
            if walk.cancelled() {
                return truncated;
            }
        }
    }
    truncated
}

pub fn run<F: FnMut(ScanProgress), C: FnMut(Target)>(
    id: &str,
    mode: ScanMode,
    selection: (Option<&str>, &[DailyCategory]),
    home: &Path,
    settings: &Settings,
    cancel: &ScanControl,
    callbacks: (F, C),
) -> Result<(ScanReport, Vec<Target>), String> {
    let (emit, record_target) = callbacks;
    let (requested_root, daily_categories) = selection;
    let start = Instant::now();
    let root = if mode == ScanMode::Full {
        let raw = Path::new(requested_root.unwrap_or(DATA_VOLUME));
        if !safety::plain_path(raw) {
            return Err("请选择有效的绝对目录路径".into());
        }
        let canonical = fs::canonicalize(raw).map_err(|e| e.to_string())?;
        if !canonical.is_dir() {
            return Err("磁盘分析需要一个目录".into());
        }
        canonical
    } else {
        home.into()
    };
    let mounts = safety::mount_paths()?;
    let mut walk = Walk {
        id,
        mode,
        cancel,
        emit,
        record_target,
        mounts,
        settings,
        entries: 0,
        errors: 0,
        skipped: 0,
        last_emit: Instant::now(),
        next_id: 0,
    };
    walk.progress(&root, true);
    let processes = safety::process_table();
    let mut targets = Vec::new();
    let mut truncated = false;
    match mode {
        ScanMode::Quick => {
            if daily_categories.is_empty() {
                return Err("请至少选择一个清理类别".into());
            }
            for rule in RULES {
                if walk.cancelled() {
                    break;
                }
                if !daily_categories.contains(&rule.category) {
                    continue;
                }
                if let Some(item) = walk.target(
                    &home.join(rule.relative),
                    rule.title,
                    daily_category(rule.category),
                    rule.owners,
                    home,
                    &processes,
                ) {
                    truncated |= walk.record(&mut targets, item);
                }
            }
            for category in [
                DailyCategory::Logs,
                DailyCategory::Temporary,
                DailyCategory::Downloads,
                DailyCategory::Trash,
            ] {
                if daily_categories.contains(&category) {
                    truncated |=
                        scan_daily_source(&mut walk, category, home, &processes, &mut targets);
                }
            }
        }
        ScanMode::Projects | ScanMode::Installers => {
            let roots = if mode == ScanMode::Projects {
                if settings.project_roots.is_empty() {
                    default_roots(home)
                } else {
                    settings.project_roots.clone()
                }
            } else {
                vec![
                    home.join("Downloads").display().to_string(),
                    home.join("Desktop").display().to_string(),
                ]
            };
            let mut found = HashSet::new();
            let mut on_found = |walk: &mut Walk<'_, F, C>, path: PathBuf| {
                if walk.cancelled() {
                    return;
                }
                let category = if mode == ScanMode::Projects {
                    "项目产物"
                } else {
                    "安装包"
                };
                let title = if mode == ScanMode::Projects {
                    format!(
                        "{} / {}",
                        path.parent()
                            .and_then(Path::file_name)
                            .unwrap_or_default()
                            .to_string_lossy(),
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                } else {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                if let Some(target) = walk.target(&path, &title, category, &[], home, &processes) {
                    truncated |= walk.record(&mut targets, target);
                }
            };
            for path in roots {
                let candidate_root = Path::new(&path);
                if !candidate_root.starts_with(home) || candidate_root == home {
                    return Err("开发目录超出用户范围，请在设置中重新指定".into());
                }
                walk.discover(
                    Path::new(&path),
                    0,
                    mode == ScanMode::Projects,
                    &mut found,
                    &mut on_found,
                );
            }
        }
        ScanMode::Full => {
            let entries =
                fs::read_dir(&root).map_err(|e| format!("无法读取 {}：{e}", root.display()))?;
            for entry in entries {
                if walk.cancelled() {
                    break;
                }
                let Ok(entry) = entry else {
                    walk.errors += 1;
                    continue;
                };
                let path = entry.path();
                if walk.skip(&path, &root) {
                    walk.skipped += 1;
                    continue;
                }
                let meta = match fs::symlink_metadata(&path) {
                    Ok(meta) if !meta.file_type().is_symlink() => meta,
                    _ => {
                        walk.skipped += 1;
                        continue;
                    }
                };
                walk.progress(&path, true);
                let measurement = walk.measure(&path);
                truncated |= walk.record(
                    &mut targets,
                    Target {
                        item: Candidate {
                            id: String::new(),
                            title: entry.file_name().to_string_lossy().into_owned(),
                            path: path.display().to_string(),
                            category: if meta.is_dir() { "目录" } else { "文件" }.into(),
                            description:
                                "双击目录逐层下钻，或在 Finder 中查看。分析结果不等于可删除数据。"
                                    .into(),
                            bytes: measurement.bytes,
                            entries: measurement.entries,
                            modified_at: measurement.latest,
                            is_dir: meta.is_dir(),
                            cleanable: false,
                            recommended: false,
                            status: if measurement.complete {
                                "locate"
                            } else {
                                "partial"
                            }
                            .into(),
                            complete: measurement.complete,
                            protection: None,
                            excluded_by: None,
                            reason: if measurement.complete {
                                "只读分析，不提供自动删除"
                            } else {
                                "仅统计可读部分，容量是下限"
                            }
                            .into(),
                        },
                        identity: None,
                        owner_patterns: vec![],
                    },
                );
            }
        }
    }
    targets.sort_by(|a, b| {
        b.item
            .bytes
            .cmp(&a.item.bytes)
            .then(a.item.path.cmp(&b.item.path))
    });
    walk.progress(&root, true);
    let report = ScanReport {
        scan_id: id.into(),
        mode,
        root: root.display().to_string(),
        parent: root.parent().map(|p| p.display().to_string()),
        disk: disk()?,
        scanned_entries: walk.entries,
        unreadable_entries: walk.errors,
        skipped_entries: walk.skipped,
        cancelled: cancel.cancelled(),
        truncated,
        elapsed_ms: start.elapsed().as_millis() as u64,
        candidates: targets.iter().map(|t| t.item.clone()).collect(),
    };
    Ok((report, targets))
}

pub struct CleanupContext<'a> {
    home: &'a Path,
    settings: &'a Settings,
    mounts: Vec<PathBuf>,
    processes: Result<String, String>,
}

impl<'a> CleanupContext<'a> {
    pub fn new(home: &'a Path, settings: &'a Settings) -> Result<Self, String> {
        Ok(Self {
            home,
            settings,
            mounts: safety::mount_paths()?,
            processes: safety::process_table(),
        })
    }
}

pub fn cleanup_precheck(target: &Target, home: &Path, settings: &Settings) -> Result<(), String> {
    if !target.item.cleanable || !target.item.complete {
        return Err(target.item.reason.clone());
    }
    let identity = target.identity.as_ref().ok_or("没有可清理的路径凭据")?;
    safety::protect(&identity.path, home, settings)?;
    safety::verify(identity)?;
    daily_policy(&identity.path, &target.item.category, home)?;
    if target.item.category == "安装包" {
        safety::installer_unmounted(&identity.path)?;
    }
    Ok(())
}

pub fn cleanup_check_with(target: &Target, context: &CleanupContext<'_>) -> Result<(), String> {
    cleanup_precheck(target, context.home, context.settings)?;
    let identity = target.identity.as_ref().ok_or("没有可清理的路径凭据")?;
    if target.item.category == "项目产物" {
        safety::project_proof(&identity.path)?;
    }
    let cancel = ScanControl::default();
    let mut walker = Walk {
        id: "validation",
        mode: ScanMode::Quick,
        cancel: &cancel,
        emit: |_| {},
        record_target: |_| {},
        mounts: context.mounts.clone(),
        settings: context.settings,
        entries: 0,
        errors: 0,
        skipped: 0,
        last_emit: Instant::now(),
        next_id: 0,
    };
    let result = walker.measure(&identity.path);
    if !result.complete || result.sensitive {
        return Err("目录已变化、含敏感文件或无法完整读取，拒绝清理".into());
    }
    safety::usage_check(&identity.path, &target.owner_patterns, &context.processes)
        .map_err(|block| block.reason)?;
    Ok(())
}

pub fn cleanup_check(target: &Target, home: &Path, settings: &Settings) -> Result<(), String> {
    cleanup_check_with(target, &CleanupContext::new(home, settings)?)
}

pub fn recheck(
    target: &Target,
    mode: ScanMode,
    home: &Path,
    settings: &Settings,
) -> Result<Target, String> {
    if mode == ScanMode::Full {
        return Err("磁盘分析结果只读，不改变删除权限".into());
    }
    let cancel = ScanControl::default();
    let mut walker = Walk {
        id: "recheck",
        mode,
        cancel: &cancel,
        emit: |_| {},
        record_target: |_| {},
        mounts: safety::mount_paths()?,
        settings,
        entries: 0,
        errors: 0,
        skipped: 0,
        last_emit: Instant::now(),
        next_id: 0,
    };
    let patterns = target
        .owner_patterns
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut updated = walker
        .target(
            Path::new(&target.item.path),
            &target.item.title,
            &target.item.category,
            &patterns,
            home,
            &safety::process_table(),
        )
        .ok_or("文件已不存在，请重新扫描")?;
    updated.item.id = target.item.id.clone();
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn installer_detection_does_not_assume_all_zips_are_installers() {
        assert!(is_installer(Path::new("a.DMG")));
        assert!(!is_installer(Path::new("photos.zip")));
    }

    #[test]
    fn measurement_tracks_activity_sensitive_files_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".env"), b"private").unwrap();
        symlink("/System", temp.path().join("link")).unwrap();
        let settings = Settings::default();
        let cancel = ScanControl::default();
        let mut walk = Walk {
            id: "test",
            mode: ScanMode::Quick,
            cancel: &cancel,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let result = walk.measure(temp.path());
        assert!(result.complete && result.sensitive);
        assert!(result.latest.is_some());
        assert_eq!(walk.entries, 2);
        assert_eq!(walk.skipped, 1);
        cancel.cancel();
        assert!(!walk.measure(temp.path()).complete);
    }

    #[test]
    fn unknown_and_excluded_paths_are_not_zero_sized_successes() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings {
            excluded_paths: vec![temp.path().display().to_string()],
            ..Default::default()
        };
        let cancel = ScanControl::default();
        let mut walk = Walk {
            id: "test",
            mode: ScanMode::Quick,
            cancel: &cancel,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        assert!(!walk.measure(&temp.path().join("missing")).complete);
        assert!(!walk.measure(temp.path()).complete);
    }

    #[test]
    fn full_scan_lists_only_children_and_supports_drilldown_and_cancel() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(home.join("folder")).unwrap();
        fs::write(home.join("folder/nested"), b"data").unwrap();
        fs::write(home.join("file"), b"data").unwrap();
        let settings = Settings::default();
        let cancel = ScanControl::default();
        let (report, targets) = run(
            "fixture",
            ScanMode::Full,
            (home.to_str(), &DailyCategory::ALL),
            &home,
            &settings,
            &cancel,
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 2);
        assert!(targets
            .iter()
            .all(|t| !t.item.cleanable && t.identity.is_none()));
        let (nested, _) = run(
            "nested",
            ScanMode::Full,
            (home.join("folder").to_str(), &DailyCategory::ALL),
            &home,
            &settings,
            &cancel,
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(nested.candidates.len(), 1);
        assert_eq!(nested.candidates[0].title, "nested");
        let (cancelled, _) = run(
            "cancel",
            ScanMode::Full,
            (home.to_str(), &DailyCategory::ALL),
            &home,
            &settings,
            &cancel,
            (|_| cancel.cancel(), |_| {}),
        )
        .unwrap();
        assert!(cancelled.cancelled);
        assert!(cancelled.candidates.iter().all(|item| !item.cleanable));
    }

    #[test]
    fn mounted_child_is_skipped_and_measurement_is_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("mount")).unwrap();
        fs::write(temp.path().join("mount/hidden"), b"data").unwrap();
        let settings = Settings::default();
        let cancel = ScanControl::default();
        let mut walk = Walk {
            id: "test",
            mode: ScanMode::Quick,
            cancel: &cancel,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![temp.path().join("mount")],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let measured = walk.measure(temp.path());
        assert!(!measured.complete);
        assert_eq!(walk.entries, 1);
        assert_eq!(walk.skipped, 1);
    }

    #[test]
    fn quick_scan_and_cleanup_check_use_same_policy_on_a_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let cache = home.join(".go/build-cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("data"), b"cache").unwrap();
        let settings = Settings::default();
        let cancel = ScanControl::default();
        let (report, targets) = run(
            "quick",
            ScanMode::Quick,
            (None, &[DailyCategory::User]),
            &home,
            &settings,
            &cancel,
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert!(!report.candidates[0].recommended);
        let target = targets[0].clone();
        assert!(target.item.cleanable, "{}", target.item.reason);
        cleanup_check(&target, &home, &settings).unwrap();
        let protected = Settings {
            excluded_paths: vec![cache.display().to_string()],
            ..Default::default()
        };
        assert!(cleanup_check(&target, &home, &protected).is_err());
        assert!(cache.exists()); // preview/validation never deletes.
    }

    #[test]
    fn quick_scan_respects_selected_categories_and_keeps_browser_personal_data_out() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let cache = home.join("Library/Caches/Google/Chrome");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("data"), b"cache").unwrap();
        let personal = home.join("Library/Application Support/Google/Chrome/Default");
        fs::create_dir_all(&personal).unwrap();
        fs::write(personal.join("History"), b"private").unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();

        let (report, _) = run(
            "browser-only",
            ScanMode::Quick,
            (None, &[DailyCategory::Browser]),
            &home,
            &settings,
            &control,
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].category, "浏览器数据");
        assert_eq!(report.candidates[0].path, cache.display().to_string());
        assert!(report
            .candidates
            .iter()
            .all(|candidate| !candidate.path.contains("Application Support")));

        assert!(run(
            "no-categories",
            ScanMode::Quick,
            (None, &[]),
            &home,
            &settings,
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .is_err());
    }

    #[test]
    fn daily_hygiene_discovers_only_old_explicit_files_and_revalidates_policy() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let downloads = home.join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let stale = downloads.join("video.crdownload");
        let complete = downloads.join("video.mp4");
        fs::write(&stale, b"partial").unwrap();
        fs::write(&complete, b"complete").unwrap();
        for path in [&stale, &complete] {
            let (ok, _) = safety::command_output(
                "/usr/bin/touch",
                &["-t", "200001010000", &path.to_string_lossy()],
                None,
            )
            .unwrap();
            assert!(ok);
        }

        let control = ScanControl::default();
        let (report, targets) = run(
            "downloads",
            ScanMode::Quick,
            (None, &[DailyCategory::Downloads]),
            &home,
            &Settings::default(),
            &control,
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].path, stale.display().to_string());
        assert_eq!(report.candidates[0].category, "下载残留");

        let renamed = downloads.join("renamed.mp4");
        fs::rename(&stale, &renamed).unwrap();
        assert!(daily_policy(&renamed, "下载残留", &home).is_err());
        assert!(cleanup_precheck(&targets[0], &home, &Settings::default()).is_err());
    }

    #[test]
    fn daily_hygiene_enforces_log_and_temporary_age_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let logs = home.join("Library/Logs/DiagnosticReports");
        fs::create_dir_all(&logs).unwrap();
        let old_log = logs.join("old.ips");
        let recent_log = logs.join("recent.ips");
        let wrong_log = logs.join("old.txt");
        for path in [&old_log, &recent_log, &wrong_log] {
            fs::write(path, b"log").unwrap();
        }
        for path in [&old_log, &wrong_log] {
            assert!(
                safety::command_output(
                    "/usr/bin/touch",
                    &["-t", "200001010000", &path.to_string_lossy()],
                    None,
                )
                .unwrap()
                .0
            );
        }
        let (logs_report, _) = run(
            "logs",
            ScanMode::Quick,
            (None, &[DailyCategory::Logs]),
            &home,
            &Settings::default(),
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(logs_report.candidates.len(), 1);
        assert_eq!(
            logs_report.candidates[0].path,
            old_log.display().to_string()
        );

        let user_temp = fs::canonicalize(std::env::temp_dir()).unwrap();
        let unique = format!("cdisk-old-{}.tmp", std::process::id());
        let old_temp = user_temp.join(unique);
        let recent_temp = user_temp.join(format!("cdisk-recent-{}.tmp", std::process::id()));
        fs::write(&old_temp, b"old").unwrap();
        fs::write(&recent_temp, b"recent").unwrap();
        assert!(
            safety::command_output(
                "/usr/bin/touch",
                &["-t", "200001010000", &old_temp.to_string_lossy()],
                None,
            )
            .unwrap()
            .0
        );
        let (temporary_report, _) = run(
            "temporary",
            ScanMode::Quick,
            (None, &[DailyCategory::Temporary]),
            &home,
            &Settings::default(),
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert!(temporary_report
            .candidates
            .iter()
            .any(|candidate| candidate.path == old_temp.display().to_string()));
        assert!(temporary_report
            .candidates
            .iter()
            .all(|candidate| candidate.path != recent_temp.display().to_string()));
        fs::remove_file(old_temp).unwrap();
        fs::remove_file(recent_temp).unwrap();
    }

    #[test]
    fn trash_items_are_individual_and_never_recommended() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let trash = home.join(".Trash");
        fs::create_dir(&trash).unwrap();
        fs::write(trash.join("old.txt"), b"trash").unwrap();
        let (report, _) = run(
            "trash",
            ScanMode::Quick,
            (None, &[DailyCategory::Trash]),
            &home,
            &Settings::default(),
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].title, "old.txt");
        assert_eq!(report.candidates[0].status, "review");
        assert!(!report.candidates[0].recommended);
    }

    #[test]
    fn daily_hygiene_rejects_matching_files_outside_approved_roots() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let document = home.join("Documents/private.log");
        fs::create_dir_all(document.parent().unwrap()).unwrap();
        fs::write(&document, b"private").unwrap();
        assert!(
            safety::command_output(
                "/usr/bin/touch",
                &["-t", "200001010000", &document.to_string_lossy()],
                None,
            )
            .unwrap()
            .0
        );
        assert!(daily_policy(&document, "日志与诊断", &home).is_err());
        assert!(daily_policy(&document, "下载残留", &home).is_err());
        assert!(daily_policy(&document, "废纸篓", &home).is_err());
    }

    #[test]
    fn preview_precheck_is_fast_but_final_check_rejects_new_sensitive_content() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let path = home.join("cache");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("data"), b"cache").unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();
        let mut walk = Walk {
            id: "precheck",
            mode: ScanMode::Quick,
            cancel: &control,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let target = walk
            .target(&path, "cache", "开发缓存", &[], &home, &Ok(String::new()))
            .unwrap();
        cleanup_precheck(&target, &home, &settings).unwrap();
        fs::write(path.join(".env"), b"secret").unwrap();
        assert!(cleanup_check(&target, &home, &settings).is_err());
        assert!(path.exists());
    }

    #[test]
    fn development_cache_rules_do_not_block_unrelated_running_runtimes() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();
        let mut walk = Walk {
            id: "rules",
            mode: ScanMode::Quick,
            cancel: &control,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let processes = Ok("/Applications/GoLand.app/Contents/MacOS/goland\n/opt/homebrew/bin/node\n/opt/homebrew/bin/bash\n/usr/bin/python\n".into());
        for rule in RULES {
            if rule.category != DailyCategory::User {
                continue;
            }
            let path = home.join(rule.relative);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("fixture"), b"cache").unwrap();
            let target = walk
                .target(
                    &path,
                    rule.title,
                    daily_category(rule.category),
                    rule.owners,
                    &home,
                    &processes,
                )
                .unwrap();
            assert!(
                target.item.cleanable,
                "{}: {}",
                rule.relative, target.item.reason
            );
            cleanup_check(&target, &home, &settings).unwrap();
            assert!(path.exists());
        }
    }

    #[test]
    fn scanner_detects_actual_open_file_and_recheck_recovers_after_close() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let path = home.join("cache");
        fs::create_dir(&path).unwrap();
        let file = fs::File::create(path.join("fixture")).unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();
        let mut walk = Walk {
            id: "occupancy",
            mode: ScanMode::Quick,
            cancel: &control,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let target = walk
            .target(&path, "cache", "开发缓存", &[], &home, &Ok(String::new()))
            .unwrap();
        assert!(
            !target.item.cleanable,
            "an open file must block the candidate"
        );
        assert_eq!(target.item.protection.as_deref(), Some("busy"));
        assert!(target.item.reason.contains(&std::process::id().to_string()));
        drop(file);
        let updated = recheck(&target, ScanMode::Quick, &home, &settings).unwrap();
        assert!(updated.item.cleanable, "{}", updated.item.reason);
        let file = fs::File::open(path.join("fixture")).unwrap();
        assert!(
            cleanup_check(&updated, &home, &settings).is_err(),
            "occupancy is revalidated before cleanup"
        );
        drop(file);
        assert!(path.exists());
    }

    #[test]
    fn cancelling_preserves_completed_candidates_but_not_the_interrupted_item() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let first = home.join("first");
        let second = home.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("data"), b"complete").unwrap();
        fs::write(second.join("data"), b"incomplete").unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();
        let mut walk = Walk {
            id: "test",
            mode: ScanMode::Quick,
            cancel: &control,
            emit: |progress: ScanProgress| {
                if progress.current_path == second.display().to_string() {
                    control.cancel();
                }
            },
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let completed = walk
            .target(&first, "first", "开发缓存", &[], &home, &Ok(String::new()))
            .unwrap();
        let interrupted = walk
            .target(
                &second,
                "second",
                "开发缓存",
                &[],
                &home,
                &Ok(String::new()),
            )
            .unwrap();
        assert!(control.cancelled());
        assert!(completed.item.complete && completed.item.cleanable);
        assert!(!interrupted.item.complete && !interrupted.item.cleanable);
        assert_eq!(interrupted.item.protection.as_deref(), Some("incomplete"));
        cleanup_check(&completed, &home, &settings).unwrap();
        assert!(cleanup_check(&interrupted, &home, &settings).is_err());
    }

    #[test]
    fn manual_protection_is_distinct_and_recheck_does_not_bypass_other_guards() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let path = home.join("cache");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("data"), b"cache").unwrap();
        let settings = Settings {
            excluded_paths: vec![path.display().to_string()],
            ..Default::default()
        };
        let control = ScanControl::default();
        let mut walk = Walk {
            id: "test",
            mode: ScanMode::Quick,
            cancel: &control,
            emit: |_| {},
            record_target: |_| {},
            mounts: vec![],
            settings: &settings,
            entries: 0,
            errors: 0,
            skipped: 0,
            last_emit: Instant::now(),
            next_id: 0,
        };
        let protected = walk
            .target(&path, "cache", "开发缓存", &[], &home, &Ok(String::new()))
            .unwrap();
        assert_eq!(protected.item.protection.as_deref(), Some("manual"));
        assert_eq!(protected.item.excluded_by.as_deref(), path.to_str());
        assert!(!protected.item.cleanable);
        let fresh = recheck(&protected, ScanMode::Quick, &home, &Settings::default()).unwrap();
        assert!(fresh.item.cleanable);
        fs::write(path.join(".env"), b"private").unwrap();
        let guarded = recheck(&fresh, ScanMode::Quick, &home, &Settings::default()).unwrap();
        assert!(!guarded.item.cleanable);
        assert_eq!(guarded.item.protection.as_deref(), Some("system"));
    }

    #[test]
    fn project_candidates_are_streamed_before_discovery_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let root = home.join("repos");
        fs::create_dir_all(root.join("one/target")).unwrap();
        fs::create_dir_all(root.join("two/target")).unwrap();
        let settings = Settings {
            project_roots: vec![root.display().to_string()],
            ..Default::default()
        };
        let control = ScanControl::default();
        let mut streamed = Vec::new();
        let (report, _) = run(
            "streamed",
            ScanMode::Projects,
            (None, &DailyCategory::ALL),
            &home,
            &settings,
            &control,
            (
                |progress| {
                    if let Some(item) = progress.candidate {
                        streamed.push(item.id);
                        control.cancel();
                    }
                },
                |_| {},
            ),
        )
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(streamed.len(), 1);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].id, streamed[0]);
        // This fixture isn't a Git repo: cancellation cannot unlock that separate restriction.
        assert!(!report.candidates[0].cleanable);
        assert!(report.candidates[0].complete);
    }

    #[test]
    fn completed_targets_stream_with_backend_identity_before_scan_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let root = home.join("repos");
        fs::create_dir_all(root.join("one/target")).unwrap();
        fs::write(root.join("one/target/data"), b"cache").unwrap();
        let settings = Settings {
            project_roots: vec![root.display().to_string()],
            ..Default::default()
        };
        let control = ScanControl::default();
        let streamed = std::cell::RefCell::new(Vec::new());
        let (report, _) = run(
            "stream-target",
            ScanMode::Projects,
            (None, &DailyCategory::ALL),
            &home,
            &settings,
            &control,
            (
                |_| {},
                |target| {
                    assert!(target.identity.is_some());
                    assert!(target.item.complete);
                    streamed.borrow_mut().push(target.item.id.clone());
                    control.cancel();
                },
            ),
        )
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(streamed.borrow().as_slice(), ["item-0"]);
    }

    #[test]
    #[ignore = "manual read-only smoke against the current Mac; never deletes or writes settings"]
    fn live_read_only_scan_smoke() {
        let home = home().unwrap();
        let settings = Settings::default();
        let cancel = ScanControl::default();
        for mode in [ScanMode::Quick, ScanMode::Installers] {
            let (report, _) = run(
                "live-smoke",
                mode,
                (None, &DailyCategory::ALL),
                &home,
                &settings,
                &cancel,
                (|_| {}, |_| {}),
            )
            .unwrap();
            if mode == ScanMode::Quick {
                assert!(report.candidates.iter().all(|candidate| {
                    candidate.category != "用户缓存"
                        || candidate.protection.as_deref() != Some("app_running")
                }));
            }
            println!(
                "{mode:?}: candidates={}, visited={}, unreadable={}, elapsed_ms={}",
                report.candidates.len(),
                report.scanned_entries,
                report.unreadable_entries,
                report.elapsed_ms
            );
            assert_eq!(report.mode, mode);
            assert!(report.disk.total_bytes > report.disk.available_bytes);
        }
        let (report, targets) = run(
            "live-cancel",
            ScanMode::Full,
            (Some(DATA_VOLUME), &DailyCategory::ALL),
            &home,
            &settings,
            &cancel,
            (
                |progress| {
                    if progress.scanned_entries > 0 {
                        cancel.cancel();
                    }
                },
                |_| {},
            ),
        )
        .unwrap();
        assert!(report.cancelled);
        assert!(targets.iter().all(|target| !target.item.cleanable));
        println!(
            "Full: cancellation verified; entries={}",
            report.scanned_entries
        );
    }
}
