use crate::models::{Identity, Settings};
use std::ffi::{CStr, CString};
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub fn command_output(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<(bool, String), String> {
    command_status(program, args, cwd).map(|(code, text)| (code == 0, text))
}

fn command_status(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
) -> Result<(i32, String), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if program == "/usr/sbin/lsof" {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let diagnostics = child.stderr.take().map(|mut pipe| {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut buffer = [0; 4096];
            let mut has_diagnostics = false;
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => has_diagnostics = true,
                    Err(_) => {
                        has_diagnostics = true;
                        break;
                    }
                }
            }
            let _ = sender.send(has_diagnostics);
        });
        receiver
    });
    let mut pipe = child.stdout.take().ok_or("无法读取命令输出")?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let n = match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            };
            // ponytail: refuse output beyond 4 MiB, rather than treating incomplete evidence as idle.
            if output.len() + n > 4 * 1024 * 1024 {
                let _ = sender.send(Err("探针输出过大，无法确认安全状态".into()));
                return;
            }
            output.extend_from_slice(&buffer[..n]);
        }
        let _ = sender.send(Ok(output));
    });
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| "读取命令输出超时")??;
                if let Some(diagnostics) = diagnostics {
                    let warned = diagnostics
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| "占用检查输出不完整")?;
                    if warned {
                        return Err("lsof 报告读取错误/警告，无法确认目录闲置".into());
                    }
                }
                return Ok((
                    status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // Do not block cancellation on a descendant inheriting the pipe.
                return Err(format!("{program} 未能在 4 秒内完成"));
            }
        }
    }
}

#[derive(Debug)]
pub struct UsageBlock {
    pub protection: &'static str,
    pub reason: String,
}

impl UsageBlock {
    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            protection: "unknown",
            reason: reason.into(),
        }
    }
}

fn lsof_usage(code: i32, output: &str) -> Result<(), UsageBlock> {
    if !matches!(code, 0 | 1) {
        return Err(UsageBlock::unknown(format!(
            "路径占用检查异常退出（{code}），未确认闲置"
        )));
    }
    if code == 1 && output.trim().is_empty() {
        return Ok(());
    }
    if output.trim().is_empty() {
        return Err(UsageBlock::unknown(
            "路径占用检查返回成功但缺少进程记录，未确认闲置",
        ));
    }
    let mut owners: Vec<(u32, String)> = Vec::new();
    let mut current = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        match line.as_bytes()[0] {
            b'p' => {
                let pid = line[1..]
                    .parse::<u32>()
                    .ok()
                    .filter(|pid| *pid > 0)
                    .ok_or_else(|| UsageBlock::unknown("占用检查返回了无效 PID，未确认闲置"))?;
                current = Some(pid);
                // ponytail: show up to 8 owners; keep validating the remaining bounded probe output.
                if owners.len() < 8 && !owners.iter().any(|(existing, _)| *existing == pid) {
                    owners.push((pid, String::new()));
                }
            }
            b'c' if current.is_some() => {
                if let Some((_, name)) = owners.iter_mut().find(|(pid, _)| Some(*pid) == current) {
                    *name = line[1..].to_string();
                }
            }
            b'f' if current.is_some() => {}
            _ => return Err(UsageBlock::unknown("占用检查输出格式不完整，未确认闲置")),
        }
    }
    if owners.is_empty() && !output.trim().is_empty() {
        return Err(UsageBlock::unknown("占用检查缺少进程记录，未确认闲置"));
    }
    let owners = owners
        .iter()
        .map(|(pid, name)| {
            if name.is_empty() {
                format!("PID {pid}")
            } else {
                format!("{name}（PID {pid}）")
            }
        })
        .collect::<Vec<_>>()
        .join("、");
    let reason = format!("该路径被 {owners} 占用；关闭对应任务后重新检查");
    Err(UsageBlock {
        protection: "busy",
        reason,
    })
}

pub fn path_idle(path: &Path) -> Result<(), UsageBlock> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| UsageBlock::unknown(format!("无法检查路径占用：{error}")))?;
    if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
        return Err(UsageBlock::unknown(
            "路径不是实体目录或普通文件，无法确认闲置",
        ));
    }
    let directory = metadata.is_dir();
    let value = path.to_string_lossy();
    let args: Vec<&str> = if directory {
        vec!["-nP", "-Fpc", "+D", &value]
    } else {
        vec!["-nP", "-Fpc", "--", &value]
    };
    let (code, output) = command_status("/usr/sbin/lsof", &args, None)
        .map_err(|error| UsageBlock::unknown(format!("无法确认路径闲置：{error}")))?;
    lsof_usage(code, &output)
}

pub fn installer_unmounted(path: &Path) -> Result<(), String> {
    if installer_is_mounted(path)? {
        return Err("安装镜像仍已挂载，请先在 Finder 推出".into());
    }
    Ok(())
}

pub fn installer_is_mounted(path: &Path) -> Result<bool, String> {
    let (ok, output) = command_output("/usr/bin/hdiutil", &["info"], None)?;
    if !ok {
        return Err("无法确认安装镜像挂载状态".into());
    }
    for line in output.lines() {
        if let Some(image) = line
            .strip_prefix("image-path")
            .and_then(|s| s.split_once(':'))
            .map(|(_, s)| s.trim())
        {
            let image = fs::canonicalize(image).unwrap_or_else(|_| PathBuf::from(image));
            if image == path {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn process_table() -> Result<String, String> {
    let (ok, text) = command_output("/bin/ps", &["-axo", "pid=,comm="], None)?;
    if !ok || text.trim().is_empty() || text.lines().any(|line| process_line(line).0.is_none()) {
        return Err("无法确认应用是否退出".into());
    }
    Ok(text)
}

fn process_line(line: &str) -> (Option<u32>, &str) {
    let trimmed = line.trim();
    if let Some((pid, command)) = trimmed.split_once(char::is_whitespace) {
        if let Ok(pid) = pid.parse::<u32>() {
            if pid > 0 && !command.trim().is_empty() {
                return (Some(pid), command.trim());
            }
        }
    }
    (None, trimmed)
}

fn matches_owner(command: &str, pattern: &str) -> bool {
    let command = command.to_lowercase();
    let pattern = pattern.trim_start_matches('/').to_lowercase();
    if pattern.ends_with(".app") {
        // Match an application bundle component, including its helper executables,
        // not arbitrary substrings in install paths such as /opt/homebrew.
        Path::new(&command)
            .components()
            .any(|part| part.as_os_str() == pattern.as_str())
    } else {
        Path::new(&command)
            .file_name()
            .is_some_and(|name| name == pattern.as_str())
    }
}

pub fn owner_idle(patterns: &[String], table: &Result<String, String>) -> Result<(), UsageBlock> {
    if patterns.is_empty() {
        return Ok(());
    }
    let text = table
        .as_ref()
        .map_err(|error| UsageBlock::unknown(error.clone()))?;
    for line in text.lines() {
        let (pid, command) = process_line(line);
        if let Some(pattern) = patterns
            .iter()
            .find(|pattern| matches_owner(command, pattern))
        {
            let owner = pid.map_or_else(|| pattern.clone(), |pid| format!("{pattern}，PID {pid}"));
            return Err(UsageBlock {
                protection: "app_running",
                reason: format!(
                    "所属应用仍在运行（{owner}）；为保护应用缓存/日志，请退出该应用后重新检查"
                ),
            });
        }
    }
    Ok(())
}

pub fn usage_check(
    path: &Path,
    owner_patterns: &[String],
    processes: &Result<String, String>,
) -> Result<(), UsageBlock> {
    owner_idle(owner_patterns, processes)?;
    path_idle(path)
}

pub fn plain_path(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().as_bytes().len() <= 1024
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        && !path
            .as_os_str()
            .as_bytes()
            .iter()
            .any(|b| b.is_ascii_control())
}

pub fn capture(path: &Path) -> Result<Identity, String> {
    if !plain_path(path) {
        return Err("只接受无跳转、无控制字符的绝对路径".into());
    }
    // Canonicalize parents as well: a symlinked Caches directory must not grant permission.
    if fs::canonicalize(path).map_err(|e| e.to_string())? != path {
        return Err("路径包含符号链接或重定向，仅供查看".into());
    }
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let parent = fs::metadata(path.parent().ok_or("缺少父目录")?).map_err(|e| e.to_string())?;
    if !(meta.is_dir() || meta.is_file()) || meta.file_type().is_symlink() {
        return Err("不是普通文件或实体目录".into());
    }
    Ok(Identity {
        path: path.into(),
        device: meta.dev(),
        inode: meta.ino(),
        parent_device: parent.dev(),
        parent_inode: parent.ino(),
    })
}

pub fn excluded(path: &Path, settings: &Settings) -> bool {
    settings.excluded_paths.iter().any(|p| path.starts_with(p))
}

pub fn protect(path: &Path, home: &Path, settings: &Settings) -> Result<(), String> {
    let volume_trash = path.starts_with("/Volumes")
        && path
            .components()
            .any(|component| component.as_os_str() == ".Trashes");
    let user_temp = fs::canonicalize(std::env::temp_dir()).ok();
    let temporary = user_temp
        .as_ref()
        .is_some_and(|directory| path.starts_with(directory) && path != directory);
    if !plain_path(path) || path == home || (!path.starts_with(home) && !volume_trash && !temporary)
    {
        return Err("系统目录、其他用户数据或用户主目录受保护".into());
    }
    if excluded(path, settings) {
        return Err("已加入保护名单".into());
    }
    let sensitive = [
        ".ssh",
        ".gnupg",
        ".aws",
        ".config",
        ".codex",
        ".claude",
        "Library/Keychains",
        "Library/Mobile Documents",
        "Library/CloudStorage",
        "Library/Messages",
        "Library/Mail",
        "Library/Containers",
        "Library/Group Containers",
        "Library/Application Support",
        "Library/LaunchAgents",
    ];
    if sensitive.iter().any(|p| path.starts_with(home.join(p)))
        || path.components().any(|p| p.as_os_str() == ".git")
    {
        return Err("账户、会话、凭证或应用数据受保护".into());
    }
    Ok(())
}

pub fn project_proof(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("无项目父目录")?;
    let (ok, root) = command_output(
        "/usr/bin/git",
        &["rev-parse", "--show-toplevel"],
        Some(parent),
    )?;
    if !ok {
        return Err("无法确认 Git 项目边界，仅供查看".into());
    }
    let root = PathBuf::from(root.trim());
    if path == root || !path.starts_with(&root) {
        return Err("仓库根目录受保护".into());
    }
    let (ok, files) = command_output(
        "/usr/bin/git",
        &["ls-files", "-z", "--", &path.to_string_lossy()],
        Some(&root),
    )?;
    if !ok || !files.is_empty() {
        return Err("包含 Git 跟踪文件或无法确认，仅供查看".into());
    }
    let (ignored, _) = command_output(
        "/usr/bin/git",
        &["check-ignore", "-q", "--", &path.to_string_lossy()],
        Some(&root),
    )?;
    if !ignored {
        return Err("未被 Git ignore 明确标记为生成物，仅供查看".into());
    }
    Ok(())
}

pub fn mount_paths() -> Result<Vec<PathBuf>, String> {
    let (ok, text) = command_output("/sbin/mount", &[], None)?;
    if !ok || text.is_empty() {
        return Err("无法读取挂载表".into());
    }
    Ok(text
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(" on ")?;
            let (mount, _) = rest.rsplit_once(" (")?;
            Some(PathBuf::from(mount))
        })
        .collect())
}

fn name(value: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| "路径包含 NUL".into())
}

fn stat_at(parent: RawFd, name: &CStr) -> Result<libc::stat, String> {
    let mut stat = std::mem::MaybeUninit::uninit();
    // SAFETY: name is NUL terminated, stat is initialized on success.
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { stat.assume_init() })
}

fn fd_stat(fd: RawFd) -> Result<libc::stat, String> {
    let mut stat = std::mem::MaybeUninit::uninit();
    // SAFETY: valid open fd, stat is initialized on success.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { stat.assume_init() })
}

fn open_dir(parent: RawFd, name: &CStr) -> Result<OwnedFd, String> {
    // SAFETY: valid relative name; O_NOFOLLOW rejects a raced symlink.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn filesystem(fd: RawFd) -> Result<[i32; 2], String> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: valid fd; stat initialized on success. fsid_t on Darwin is two i32.
    if unsafe { libc::fstatfs(fd, stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(stat.f_fsid) })
}

fn delete_entry(
    parent: RawFd,
    entry: &CStr,
    expected: &libc::stat,
    fsid: [i32; 2],
) -> Result<(), String> {
    let current = stat_at(parent, entry)?;
    if current.st_dev != expected.st_dev || current.st_ino != expected.st_ino {
        return Err("删除期间对象发生变化，已停止".into());
    }
    if current.st_mode & libc::S_IFMT == libc::S_IFDIR {
        let child = open_dir(parent, entry)?;
        let opened = fd_stat(child.as_raw_fd())?;
        if opened.st_ino != current.st_ino
            || opened.st_dev != current.st_dev
            || filesystem(child.as_raw_fd())? != fsid
        {
            return Err("检测到路径替换或挂载点，已停止".into());
        }
        // SAFETY: fdopendir owns the duplicated descriptor.
        let dup = unsafe { libc::dup(child.as_raw_fd()) };
        if dup < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let dir = unsafe { libc::fdopendir(dup) };
        if dir.is_null() {
            unsafe {
                libc::close(dup);
            }
            return Err(std::io::Error::last_os_error().to_string());
        }
        let result = (|| {
            loop {
                // SAFETY: Darwin errno and DIR are thread local / valid until closed below.
                unsafe {
                    *libc::__error() = 0;
                }
                let item = unsafe { libc::readdir(dir) };
                if item.is_null() {
                    if unsafe { *libc::__error() } != 0 {
                        return Err(std::io::Error::last_os_error().to_string());
                    }
                    break;
                }
                let item = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) };
                if matches!(item.to_bytes(), b"." | b"..") {
                    continue;
                }
                let meta = stat_at(child.as_raw_fd(), item)?;
                delete_entry(child.as_raw_fd(), item, &meta, fsid)?;
            }
            Ok::<_, String>(())
        })();
        unsafe {
            libc::closedir(dir);
        }
        result?;
    }
    let current = stat_at(parent, entry)?;
    if current.st_ino != expected.st_ino || current.st_dev != expected.st_dev {
        return Err("删除期间对象发生变化，已停止".into());
    }
    let flags = if current.st_mode & libc::S_IFMT == libc::S_IFDIR {
        libc::AT_REMOVEDIR
    } else {
        0
    };
    // SAFETY: unlinkat removes only this entry; symlinks are unlinked, never traversed.
    if unsafe { libc::unlinkat(parent, entry.as_ptr(), flags) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

pub fn verify(identity: &Identity) -> Result<(), String> {
    let now = capture(&identity.path)?;
    if (now.device, now.inode, now.parent_device, now.parent_inode)
        != (
            identity.device,
            identity.inode,
            identity.parent_device,
            identity.parent_inode,
        )
    {
        return Err("扫描后目标或父目录已被替换，请重新扫描".into());
    }
    Ok(())
}

pub fn remove(identity: &Identity, ticket: &str) -> Result<(), String> {
    verify(identity)?;
    let path = &identity.path;
    if mount_paths()?.iter().any(|mount| mount.starts_with(path)) {
        return Err("目标包含挂载点".into());
    }
    let parent = open_dir(
        libc::AT_FDCWD,
        &name(path.parent().ok_or("缺少父目录")?.as_os_str())?,
    )?;
    let meta = fd_stat(parent.as_raw_fd())?;
    if meta.st_dev as u64 != identity.parent_device || meta.st_ino != identity.parent_inode {
        return Err("父目录发生变化".into());
    }
    let leaf = name(path.file_name().ok_or("缺少名称")?)?;
    let quarantine = CString::new(format!(".cdisk-{ticket}")).map_err(|_| "无效操作 ID")?;
    // SAFETY: same verified parent, exclusive destination; cannot overwrite existing files.
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            parent.as_raw_fd(),
            quarantine.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let result = (|| {
        let moved = stat_at(parent.as_raw_fd(), &quarantine)?;
        if moved.st_dev as u64 != identity.device || moved.st_ino != identity.inode {
            return Err("隔离后身份不匹配，未删除".into());
        }
        delete_entry(
            parent.as_raw_fd(),
            &quarantine,
            &moved,
            filesystem(parent.as_raw_fd())?,
        )
    })();
    if let Err(error) = result {
        // Never overwrite a directory that an active application has recreated.
        let restored = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                quarantine.as_ptr(),
                parent.as_raw_fd(),
                leaf.as_ptr(),
                libc::RENAME_EXCL,
            )
        } == 0;
        return Err(if restored {
            format!("{error}；可能已部分清理，剩余内容已放回")
        } else {
            format!(
                "{error}；剩余内容保留在 {}",
                path.parent()
                    .unwrap()
                    .join(quarantine.to_string_lossy().as_ref())
                    .display()
            )
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn rejects_symlink_ancestor_and_root() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("actual")).unwrap();
        symlink(temp.path().join("actual"), temp.path().join("alias")).unwrap();
        assert!(capture(&temp.path().join("alias")).is_err());
        fs::write(temp.path().join("actual/data"), b"keep").unwrap();
        assert!(capture(&temp.path().join("alias/data")).is_err());
    }

    #[test]
    fn refuses_root_and_sensitive_and_exclusions() {
        let home = Path::new("/Users/example");
        let settings = Settings {
            excluded_paths: vec!["/Users/example/keep".into()],
            ..Default::default()
        };
        for path in [
            "/",
            "/System",
            "/Users/example",
            "/Users/example/.ssh/id",
            "/Users/example/project/.git/index",
            "/Users/example/keep/cache",
        ] {
            assert!(protect(Path::new(path), home, &settings).is_err());
        }
        assert!(protect(&home.join("Library/Caches/pip"), home, &settings).is_ok());
        assert!(!plain_path(Path::new("/Users/example/../root")));
    }

    #[test]
    fn removes_only_captured_directory_and_not_symlink_contents() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        fs::write(root.join("keep"), b"keep").unwrap();
        fs::write(root.join("cache/data"), b"data").unwrap();
        symlink(root.join("keep"), root.join("cache/link")).unwrap();
        remove(&capture(&root.join("cache")).unwrap(), "fixture").unwrap();
        assert!(!root.join("cache").exists());
        assert_eq!(fs::read(root.join("keep")).unwrap(), b"keep");
    }

    #[test]
    fn replacement_does_not_get_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        let identity = capture(&root.join("cache")).unwrap();
        fs::rename(root.join("cache"), root.join("original")).unwrap();
        fs::create_dir(root.join("cache")).unwrap();
        assert!(remove(&identity, "fixture").is_err());
        assert!(root.join("cache").exists());
    }

    #[test]
    fn running_and_unknown_owners_are_not_idle() {
        let patterns = vec!["goland".into()];
        assert!(owner_idle(&patterns, &Ok("/applications/goland.app/goland".into())).is_err());
        assert!(owner_idle(&patterns, &Err("no ps".into())).is_err());
        assert!(owner_idle(&patterns, &Ok("/bin/bash".into())).is_ok());
        assert!(owner_idle(
            &["/go".into()],
            &Ok("/applications/google chrome.app/chrome".into())
        )
        .is_ok());
        assert!(owner_idle(&["/go".into()], &Ok("/usr/local/bin/go".into())).is_err());
    }

    #[test]
    fn open_file_is_not_idle() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("in-use");
        let file = fs::File::create(&path).unwrap();
        assert!(path_idle(&path).is_err());
        drop(file);
        path_idle(&path).unwrap();
    }

    #[test]
    fn runtime_installation_prefix_is_not_an_owner_identity() {
        let processes = Ok(
            "/opt/homebrew/bin/bash\n/opt/homebrew/bin/node\n/opt/puppetlabs/puppet/bin/ruby\n"
                .into(),
        );
        assert!(owner_idle(&["brew".into()], &processes).is_ok());
        assert!(owner_idle(&["node".into()], &Ok("/tools/node_repl".into())).is_ok());
    }

    #[test]
    fn app_owner_matches_bundle_component_and_reports_pid_not_arbitrary_paths() {
        let owners = vec!["Lark.app".into()];
        let unrelated = Ok("101 /opt/homebrew/bin/node\n102 /tools/lark-cli\n103 /Applications/NotLark.app/Contents/MacOS/lark\n".into());
        assert!(owner_idle(&owners, &unrelated).is_ok());
        let helper = Ok("321 /Applications/Lark.app/Contents/Frameworks/Lark Helper.app/Contents/MacOS/Lark Helper\n".into());
        let block = owner_idle(&owners, &helper).unwrap_err();
        assert_eq!(block.protection, "app_running");
        assert!(block.reason.contains("PID 321"));
        assert_eq!(
            owner_idle(&owners, &Err("ps failed".into()))
                .unwrap_err()
                .protection,
            "unknown"
        );
    }

    #[test]
    fn lsof_uses_both_exit_status_and_process_records() {
        assert!(lsof_usage(1, "").is_ok());
        for code in [0, 1] {
            let block =
                lsof_usage(code, "p123\ncnode\nf4\np456\ncGoLand Helper\nf5\n").unwrap_err();
            assert_eq!(block.protection, "busy");
            assert!(block.reason.contains("node（PID 123）"));
            assert!(block.reason.contains("GoLand Helper（PID 456）"));
        }
        assert_eq!(lsof_usage(0, "").unwrap_err().protection, "unknown");
        for (code, output) in [
            (2, ""),
            (-1, ""),
            (1, "pINVALID\n"),
            (1, "cnode\n"),
            (1, "unexpected\n"),
        ] {
            assert_eq!(lsof_usage(code, output).unwrap_err().protection, "unknown");
        }
    }

    #[test]
    fn unavailable_path_is_unknown_not_busy_or_idle() {
        let temp = tempfile::tempdir().unwrap();
        let error = path_idle(&temp.path().join("missing")).unwrap_err();
        assert_eq!(error.protection, "unknown");
    }

    #[test]
    fn parent_replacement_is_rejected_and_quarantine_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir_all(root.join("parent/cache")).unwrap();
        let identity = capture(&root.join("parent/cache")).unwrap();
        fs::rename(root.join("parent"), root.join("old")).unwrap();
        fs::create_dir_all(root.join("parent/cache")).unwrap();
        assert!(remove(&identity, "check").is_err());
        let identity = capture(&root.join("parent/cache")).unwrap();
        fs::write(root.join("parent/.cdisk-check"), b"preserve").unwrap();
        assert!(remove(&identity, "check").is_err());
        assert_eq!(
            fs::read(root.join("parent/.cdisk-check")).unwrap(),
            b"preserve"
        );
        assert!(root.join("parent/cache").exists());
    }
}
