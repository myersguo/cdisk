use crate::models::{HistoryEntry, Settings};
use crate::safety;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SETTING_PATHS: usize = 256;

#[derive(Clone)]
pub struct Storage(pub PathBuf);

impl Storage {
    fn ensure_private_dir(&self) -> Result<(), String> {
        if !self.0.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.0)
                .map_err(|error| error.to_string())?;
        }
        let metadata = fs::symlink_metadata(&self.0).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("配置目录不能是符号链接".into());
        }
        if fs::canonicalize(&self.0).map_err(|error| error.to_string())? != self.0 {
            return Err("配置目录的父路径包含重定向，拒绝访问".into());
        }
        fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())
    }

    fn read<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, String> {
        self.ensure_private_dir()?;
        let path = self.0.join(name);
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
            Err(error) => return Err(error.to_string()),
        };
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
            return Err(format!("{} 不是有效的小型配置文件", path.display()));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err(format!("{} 不是有效的小型配置文件", path.display()));
        }
        serde_json::from_slice(&bytes).map_err(|error| format!("{} 损坏：{error}", path.display()))
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        self.ensure_private_dir()?;
        let destination = self.0.join(name);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(format!("{name} 不是普通文件，拒绝覆盖"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let temporary = self
            .0
            .join(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_JSON_BYTES {
            return Err(format!("{name} 超过本地存储上限"));
        }
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(&temporary, &destination).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn settings(&self) -> Result<Settings, String> {
        let settings: Settings = self.read("settings.json")?;
        if settings.project_roots.len() + settings.excluded_paths.len() > MAX_SETTING_PATHS {
            return Err("设置包含过多路径，请修复 settings.json".into());
        }
        for path in settings
            .project_roots
            .iter()
            .chain(&settings.excluded_paths)
        {
            if !safety::plain_path(Path::new(path)) {
                return Err("设置包含无效路径，请修复 settings.json".into());
            }
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings, home: &Path) -> Result<Settings, String> {
        let mut settings = settings.clone();
        if settings.project_roots.len() + settings.excluded_paths.len() > MAX_SETTING_PATHS {
            return Err("扫描与保护路径总数不能超过 256 条".into());
        }
        for path in settings
            .project_roots
            .iter_mut()
            .chain(settings.excluded_paths.iter_mut())
        {
            if path.starts_with("~/") {
                *path = home.join(&path[2..]).display().to_string();
            }
            let parsed = Path::new(path);
            if !safety::plain_path(parsed) {
                return Err(format!("无效路径：{path}"));
            }
        }
        for path in &mut settings.project_roots {
            let canonical =
                fs::canonicalize(&*path).map_err(|_| format!("开发目录不存在：{path}"))?;
            if !canonical.is_dir() || !canonical.starts_with(home) || canonical == home {
                return Err("开发目录必须是用户目录内的子目录，不能是整个主目录".into());
            }
            *path = canonical.display().to_string();
        }
        settings.project_roots.sort();
        settings.project_roots.dedup();
        let roots = settings.project_roots.clone();
        settings
            .project_roots
            .retain(|p| !roots.iter().any(|r| r != p && Path::new(p).starts_with(r)));
        settings.excluded_paths.sort();
        settings.excluded_paths.dedup();
        for path in &mut settings.excluded_paths {
            if let Ok(canonical) = fs::canonicalize(&*path) {
                *path = canonical.display().to_string();
            }
        }
        settings.excluded_paths.sort();
        settings.excluded_paths.dedup();
        self.write("settings.json", &settings)?;
        Ok(settings)
    }

    pub fn history(&self) -> Result<Vec<HistoryEntry>, String> {
        self.read("history.json")
    }

    pub fn record(&self, entry: &HistoryEntry) -> Result<(), String> {
        let mut history = self.history()?;
        history.retain(|old| old.id != entry.id);
        history.insert(0, entry.clone());
        // ponytail: local UI keeps the most recent 100 operations; add export/rotation before retaining more.
        history.truncate(100);
        while history.len() > 1
            && serde_json::to_vec_pretty(&history)
                .is_ok_and(|bytes| bytes.len() as u64 > MAX_JSON_BYTES)
        {
            history.pop();
        }
        self.write("history.json", &history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ItemOutcome;

    #[test]
    fn history_updates_one_record_and_corruption_is_not_silently_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let store = Storage(fs::canonicalize(temp.path()).unwrap().join("state"));
        let mut entry = HistoryEntry {
            id: "op".into(),
            started_at: 1,
            status: "running".into(),
            estimated_bytes: 5,
            available_before: 10,
            available_after: None,
            items: vec![ItemOutcome {
                title: "cache".into(),
                path: "/example".into(),
                status: "pending".into(),
                message: String::new(),
            }],
        };
        store.record(&entry).unwrap();
        entry.status = "complete".into();
        store.record(&entry).unwrap();
        assert_eq!(store.history().unwrap().len(), 1);
        assert_eq!(store.history().unwrap()[0].status, "complete");
        fs::write(store.0.join("history.json"), b"broken").unwrap();
        assert!(store.record(&entry).is_err());
    }

    #[test]
    fn settings_validate_roots_and_remove_overlap() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir_all(home.join("repos/a")).unwrap();
        let store = Storage(home.join("state"));
        let settings = Settings {
            project_roots: vec!["~/repos".into(), "~/repos/a".into()],
            excluded_paths: vec!["~/keep".into()],
        };
        assert_eq!(
            store
                .save_settings(&settings, &home)
                .unwrap()
                .project_roots
                .len(),
            1
        );
        assert!(store
            .save_settings(
                &Settings {
                    project_roots: vec!["/".into()],
                    ..Default::default()
                },
                &home
            )
            .is_err());
    }

    #[test]
    fn settings_reject_excessive_paths_and_storage_rejects_symlinks_or_large_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        let store = Storage(home.join("state"));
        let settings = Settings {
            project_roots: vec![],
            excluded_paths: (0..=MAX_SETTING_PATHS)
                .map(|index| home.join(format!("keep-{index}")).display().to_string())
                .collect(),
        };
        assert!(store.save_settings(&settings, &home).is_err());

        fs::create_dir(&store.0).unwrap();
        let outside = home.join("outside.json");
        fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(&outside, store.0.join("settings.json")).unwrap();
        assert!(store.settings().is_err());
        fs::remove_file(store.0.join("settings.json")).unwrap();

        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(store.0.join("settings.json"))
            .unwrap();
        file.set_len(MAX_JSON_BYTES + 1).unwrap();
        assert!(store.settings().is_err());
    }
}
