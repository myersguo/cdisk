use crate::{models::*, safety, scanner, task::ScanControl};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const ROOT_ID: &str = "analysis-root";

struct Node {
    target: Target,
    parent: Option<usize>,
    depth: usize,
    pending: usize,
    enumerated: bool,
    valid: bool,
    sensitive: bool,
}

struct Tree<'a, F, C> {
    id: &'a str,
    home: &'a Path,
    settings: &'a Settings,
    control: &'a ScanControl,
    mounts: Vec<PathBuf>,
    root_identity: Identity,
    nodes: Vec<Node>,
    queue: VecDeque<usize>,
    focus: Option<PathBuf>,
    hardlinks: HashSet<(u64, u64)>,
    dirty: HashSet<usize>,
    emit: F,
    record: C,
    last_emit: Instant,
    errors: u64,
    skipped: u64,
}

impl<F: FnMut(ScanProgress), C: FnMut(Target)> Tree<'_, F, C> {
    fn publish(&mut self, path: &Path, force: bool) {
        if !force && self.last_emit.elapsed().as_millis() < 120 {
            return;
        }
        for index in self.dirty.drain() {
            (self.record)(self.nodes[index].target.clone());
        }
        (self.emit)(ScanProgress {
            scan_id: self.id.into(),
            mode: ScanMode::Full,
            scanned_entries: self.nodes.len() as u64,
            unreadable_entries: self.errors,
            current_path: path.display().to_string(),
            candidate: None,
        });
        self.last_emit = Instant::now();
    }

    fn invalidate(&mut self, mut index: usize) {
        loop {
            self.nodes[index].valid = false;
            self.dirty.insert(index);
            match self.nodes[index].parent {
                Some(parent) => index = parent,
                None => break,
            }
        }
    }

    fn add(&mut self, path: &Path, meta: &fs::Metadata, parent: Option<usize>) -> usize {
        let index = self.nodes.len();
        let identity = parent
            .map(|parent| {
                let identity = self.nodes[parent].target.identity.as_ref().unwrap();
                Identity {
                    path: path.into(),
                    device: meta.dev(),
                    inode: meta.ino(),
                    parent_device: identity.device,
                    parent_inode: identity.inode,
                }
            })
            .unwrap_or_else(|| self.root_identity.clone());
        let sensitive = scanner::sensitive_path(path);
        let mut size = meta.blocks().saturating_mul(512);
        if meta.is_file() && meta.nlink() > 1 && !self.hardlinks.insert((meta.dev(), meta.ino())) {
            size = 0;
        }
        let complete = !meta.is_dir();
        self.nodes.push(Node {
            target: Target {
                item: Candidate {
                    id: if parent.is_none() {
                        ROOT_ID.into()
                    } else {
                        format!("item-{index}")
                    },
                    title: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    path: path.display().to_string(),
                    category: if meta.is_dir() { "目录" } else { "文件" }.into(),
                    description: "点击目录在右侧展开子项；手动选择不需要的内容并移到废纸篓。"
                        .into(),
                    bytes: size,
                    entries: 1,
                    modified_at: u64::try_from(meta.mtime()).ok(),
                    is_dir: meta.is_dir(),
                    cleanable: false,
                    recommended: false,
                    status: "partial".into(),
                    reason: "目录含敏感内容或扫描不完整，不能移到废纸篓".into(),
                    protection: None,
                    excluded_by: None,
                    complete,
                },
                identity: Some(identity),
                owner_patterns: vec![],
            },
            parent,
            depth: parent.map_or(0, |p| self.nodes[p].depth + 1),
            pending: 0,
            enumerated: complete,
            valid: true,
            sensitive,
        });
        if meta.is_dir() {
            if self
                .focus
                .as_ref()
                .is_some_and(|focus| path.starts_with(focus))
            {
                self.queue.push_front(index);
            } else {
                self.queue.push_back(index);
            }
            if let Some(parent) = parent {
                self.nodes[parent].pending += 1;
            }
        } else {
            self.finish_item(index);
        }
        let modified = self.nodes[index].target.item.modified_at;
        let mut ancestor = parent;
        while let Some(parent) = ancestor {
            let node = &mut self.nodes[parent];
            node.target.item.bytes = node.target.item.bytes.saturating_add(size);
            node.target.item.entries += 1;
            node.target.item.modified_at = node.target.item.modified_at.max(modified);
            node.sensitive |= sensitive;
            self.dirty.insert(parent);
            ancestor = node.parent;
        }
        self.dirty.insert(index);
        index
    }

    fn finish_item(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        let item = &mut node.target.item;
        item.complete = node.valid;
        let eligibility = if node.parent.is_none() || !Path::new(&item.path).starts_with(self.home)
        {
            Err("系统目录、其他用户数据或用户主目录受保护".into())
        } else if !node.valid || node.sensitive {
            Err("目录含敏感内容或扫描不完整，不能移到废纸篓".into())
        } else {
            safety::trash_protect(Path::new(&item.path), self.home, self.settings)
        };
        item.cleanable = eligibility.is_ok();
        item.status = if item.cleanable {
            "review"
        } else if item.complete {
            "protected"
        } else {
            "partial"
        }
        .into();
        item.reason = eligibility
            .err()
            .unwrap_or_else(|| "手动选择后移到废纸篓，可在 Finder 中恢复".into());
        self.dirty.insert(index);
    }

    fn finish_directory(&mut self, mut index: usize) {
        loop {
            if !self.nodes[index].enumerated || self.nodes[index].pending != 0 {
                break;
            }
            self.finish_item(index);
            match self.nodes[index].parent {
                Some(parent) => {
                    self.nodes[parent].pending -= 1;
                    index = parent;
                }
                None => break,
            }
        }
    }

    fn enumerate(&mut self, index: usize) {
        let path = PathBuf::from(&self.nodes[index].target.item.path);
        // ponytail: cap depth at 128; deeper trees need a reviewed traversal/protection policy.
        let identity = self.nodes[index].target.identity.as_ref().unwrap();
        let unchanged = fs::canonicalize(&path).is_ok_and(|canonical| canonical == path)
            && fs::symlink_metadata(&path).is_ok_and(|meta| {
                meta.dev() == identity.device && meta.ino() == identity.inode && meta.is_dir()
            });
        if self.nodes[index].depth >= 128 || !unchanged {
            self.skipped += 1;
            self.invalidate(index);
        } else if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries {
                if !self.control.checkpoint() {
                    self.invalidate(index);
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        self.errors += 1;
                        self.invalidate(index);
                        continue;
                    }
                };
                let child = entry.path();
                if child.file_name().is_some_and(|name| name == ".git") {
                    let mut ancestor = Some(index);
                    while let Some(index) = ancestor {
                        self.nodes[index].sensitive = true;
                        self.dirty.insert(index);
                        ancestor = self.nodes[index].parent;
                    }
                }
                if safety::excluded(&child, self.settings)
                    || self.mounts.contains(&child)
                    || child == Path::new("/System/Volumes/Data/Volumes")
                {
                    self.skipped += 1;
                    self.invalidate(index);
                    continue;
                }
                match fs::symlink_metadata(&child) {
                    Ok(meta)
                        if !meta.file_type().is_symlink()
                            && (meta.is_file() || meta.is_dir())
                            && meta.dev()
                                == self.nodes[index].target.identity.as_ref().unwrap().device =>
                    {
                        self.add(&child, &meta, Some(index));
                    }
                    Ok(meta) => {
                        self.skipped += 1;
                        if meta.dev() != self.nodes[index].target.identity.as_ref().unwrap().device
                        {
                            self.invalidate(index);
                        }
                    }
                    Err(_) => {
                        self.errors += 1;
                        self.invalidate(index);
                    }
                }
                self.publish(&child, false);
            }
        } else {
            self.errors += 1;
            self.invalidate(index);
        }
        self.nodes[index].enumerated = true;
        self.finish_directory(index);
        self.publish(&path, index == 0);
    }
}

pub fn run<F: FnMut(ScanProgress), C: FnMut(Target)>(
    id: &str,
    root: &Path,
    home: &Path,
    settings: &Settings,
    control: &ScanControl,
    callbacks: (F, C),
) -> Result<(ScanReport, Vec<Target>), String> {
    let start = Instant::now();
    let meta = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    let parent = fs::metadata(root.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
    let root_identity = Identity {
        path: root.into(),
        device: meta.dev(),
        inode: meta.ino(),
        parent_device: parent.dev(),
        parent_inode: parent.ino(),
    };
    let mut tree = Tree {
        id,
        home,
        settings,
        control,
        mounts: safety::mount_paths()?,
        root_identity,
        nodes: vec![],
        queue: VecDeque::new(),
        focus: None,
        hardlinks: HashSet::new(),
        dirty: HashSet::new(),
        emit: callbacks.0,
        record: callbacks.1,
        last_emit: Instant::now(),
        errors: 0,
        skipped: 0,
    };
    tree.add(root, &meta, None);
    tree.publish(root, true);
    while !tree.queue.is_empty() && control.checkpoint() {
        if let Some(priority) = control.take_priority() {
            let index = if priority == ROOT_ID {
                Some(0)
            } else {
                priority
                    .strip_prefix("item-")
                    .and_then(|id| id.parse::<usize>().ok())
            };
            if let Some(node) = index.and_then(|index| tree.nodes.get(index)) {
                let path = PathBuf::from(&node.target.item.path);
                if tree.focus.as_ref() != Some(&path) {
                    let (mut selected, mut remaining): (VecDeque<_>, VecDeque<_>) =
                        tree.queue.drain(..).partition(|index| {
                            Path::new(&tree.nodes[*index].target.item.path).starts_with(&path)
                        });
                    selected.append(&mut remaining);
                    tree.queue = selected;
                    tree.focus = Some(path);
                }
            }
        }
        let index = tree.queue.pop_front().unwrap();
        tree.enumerate(index);
    }
    tree.publish(root, true);
    let mut candidates = tree
        .nodes
        .iter()
        .filter(|node| node.parent == Some(0))
        .map(|node| node.target.item.clone())
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    let truncated = candidates.len() > scanner::MAX_RESULTS;
    candidates.truncate(scanner::MAX_RESULTS);
    let report = ScanReport {
        scan_id: id.into(),
        mode: ScanMode::Full,
        root: root.display().to_string(),
        parent: root.parent().map(|p| p.display().to_string()),
        disk: scanner::disk()?,
        scanned_entries: tree.nodes.len() as u64,
        unreadable_entries: tree.errors,
        skipped_entries: tree.skipped,
        cancelled: control.cancelled(),
        truncated,
        elapsed_ms: start.elapsed().as_millis() as u64,
        candidates,
    };
    Ok((
        report,
        tree.nodes.into_iter().map(|node| node.target).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_continues_past_250_000_retained_nodes() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(home.join("child")).unwrap();
        fs::write(home.join("child/file"), b"fixture").unwrap();
        let meta = fs::metadata(&home).unwrap();
        let parent = fs::metadata(home.parent().unwrap()).unwrap();
        let settings = Settings::default();
        let control = ScanControl::default();
        let mut tree = Tree {
            id: "large-tree",
            home: &home,
            settings: &settings,
            control: &control,
            mounts: vec![],
            root_identity: Identity {
                path: home.clone(),
                device: meta.dev(),
                inode: meta.ino(),
                parent_device: parent.dev(),
                parent_inode: parent.ino(),
            },
            nodes: vec![],
            queue: VecDeque::new(),
            focus: None,
            hardlinks: HashSet::new(),
            dirty: HashSet::new(),
            emit: |_| {},
            record: |_| {},
            last_emit: Instant::now(),
            errors: 0,
            skipped: 0,
        };
        tree.add(&home, &meta, None);
        // Seed retained nodes without creating 250,000 filesystem entries.
        let mut item = tree.nodes[0].target.item.clone();
        item.id.clear();
        item.title.clear();
        item.path.clear();
        item.category.clear();
        item.description.clear();
        item.status.clear();
        item.reason.clear();
        tree.nodes.resize_with(250_000, || Node {
            target: Target {
                item: item.clone(),
                identity: None,
                owner_patterns: vec![],
            },
            parent: None,
            depth: 0,
            pending: 0,
            enumerated: true,
            valid: true,
            sensitive: false,
        });
        while let Some(index) = tree.queue.pop_front() {
            tree.enumerate(index);
        }
        assert_eq!(tree.nodes.len(), 250_002);
        assert_eq!(tree.errors, 0);
        assert_eq!(tree.skipped, 0);
        assert!(tree.nodes[0].target.item.complete);
        assert!(tree.nodes[250_000].target.item.complete);
        let file = &tree.nodes[250_001].target.item;
        assert_eq!(file.path, home.join("child/file").display().to_string());
        assert!(file.complete && file.cleanable);
    }

    #[test]
    fn cancellation_keeps_discovered_directories_and_completed_files() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(home.join("pending")).unwrap();
        fs::write(home.join("pending/child"), b"pending").unwrap();
        fs::write(home.join("finished.txt"), b"finished").unwrap();
        let control = ScanControl::default();
        let (report, _) = run(
            "cancel-tree",
            &home,
            &home,
            &Settings::default(),
            &control,
            (
                |progress| {
                    if progress.scanned_entries >= 3 {
                        control.cancel();
                    }
                },
                |_| {},
            ),
        )
        .unwrap();
        assert!(report.cancelled);
        let pending = report.candidates.iter().find(|item| item.is_dir).unwrap();
        assert!(!pending.complete && !pending.cleanable);
        let finished = report.candidates.iter().find(|item| !item.is_dir).unwrap();
        assert!(finished.complete && finished.cleanable);
    }

    #[test]
    fn skipped_content_and_git_symlinks_protect_their_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir_all(home.join("excluded/keep")).unwrap();
        fs::create_dir_all(home.join("repo")).unwrap();
        std::os::unix::fs::symlink(home.join("excluded/keep"), home.join("repo/.git")).unwrap();
        let settings = Settings {
            excluded_paths: vec![home.join("excluded/keep").display().to_string()],
            ..Default::default()
        };
        let (report, _) = run(
            "protected-tree",
            &home,
            &home,
            &settings,
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 2);
        assert!(report.candidates.iter().all(|item| !item.cleanable));
        assert!(
            !report
                .candidates
                .iter()
                .find(|item| item.title == "excluded")
                .unwrap()
                .complete
        );
    }
}
