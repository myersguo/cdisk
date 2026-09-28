mod analysis;
mod models;
mod safety;
mod scanner;
mod storage;
mod task;

use models::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use storage::Storage;
use task::ScanControl;
use tauri::{Emitter, Manager};

struct Snapshot {
    id: String,
    mode: ScanMode,
    targets: HashMap<String, Target>,
    children: HashMap<String, Vec<String>>,
}

impl Snapshot {
    fn record(&mut self, target: Target) {
        if self.mode == ScanMode::Full && !self.targets.contains_key(&target.item.id) {
            if let Some(parent) = Path::new(&target.item.path).parent() {
                self.children
                    .entry(parent.display().to_string())
                    .or_default()
                    .push(target.item.id.clone());
            }
        }
        self.targets.insert(target.item.id.clone(), target);
    }
}

struct Plan {
    token: String,
    scan_id: String,
    trash: bool,
    targets: Vec<Target>,
    created: Instant,
}

struct Preparation {
    id: String,
    control: Arc<ScanControl>,
}

#[derive(Default)]
struct Data {
    snapshots: HashMap<ScanMode, Snapshot>,
    pending: Option<Plan>,
    scans: HashMap<ScanMode, (String, Arc<ScanControl>)>,
    preparation: Option<Preparation>,
    mutation: bool,
}

#[derive(Default)]
struct State {
    data: Mutex<Data>,
}

struct Guard {
    state: Arc<State>,
    scan: Option<(ScanMode, String)>,
    mutation: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut data) = self.state.data.lock() {
            if let Some((mode, id)) = &self.scan {
                if data
                    .scans
                    .get(mode)
                    .is_some_and(|(current, _)| current == id)
                {
                    data.scans.remove(mode);
                }
            } else if self.mutation {
                data.mutation = false;
                data.preparation = None;
            }
        }
    }
}

fn begin(state: &Arc<State>, _id: &str) -> Result<Guard, String> {
    let mut data = state.data.lock().map_err(|_| "状态不可用")?;
    if data.mutation {
        return Err("已有清理或设置操作进行中".into());
    }
    data.mutation = true;
    Ok(Guard {
        state: Arc::clone(state),
        scan: None,
        mutation: true,
    })
}

fn begin_preparation(state: &Arc<State>, id: &str) -> Result<(Guard, Arc<ScanControl>), String> {
    let guard = begin(state, id)?;
    let control = Arc::new(ScanControl::default());
    state.data.lock().map_err(|_| "状态不可用")?.preparation = Some(Preparation {
        id: id.into(),
        control: Arc::clone(&control),
    });
    Ok((guard, control))
}

fn begin_scan(
    state: &Arc<State>,
    mode: ScanMode,
    id: &str,
) -> Result<(Guard, Arc<ScanControl>), String> {
    let mut data = state.data.lock().map_err(|_| "状态不可用")?;
    if data.mutation || data.scans.contains_key(&mode) {
        return Err("当前页已有扫描或正在修改数据，请稍后重试".into());
    }
    if data.scans.values().any(|(current, _)| current == id)
        || data.snapshots.values().any(|snapshot| snapshot.id == id)
    {
        return Err("扫描 ID 已使用".into());
    }
    let control = Arc::new(ScanControl::default());
    data.scans.insert(mode, (id.into(), Arc::clone(&control)));
    data.snapshots.remove(&mode);
    // A scan on another page must not invalidate a prepared plan.
    if data
        .pending
        .as_ref()
        .is_some_and(|plan| !data.snapshots.values().any(|s| s.id == plan.scan_id))
    {
        data.pending = None;
    }
    Ok((
        Guard {
            state: Arc::clone(state),
            scan: Some((mode, id.into())),
            mutation: false,
        },
        control,
    ))
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("操作 ID 无效".into());
    }
    Ok(())
}

fn validate_candidate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("候选 ID 无效".into());
    }
    Ok(())
}

fn validate_daily_categories(
    mode: ScanMode,
    categories: Option<Vec<DailyCategory>>,
) -> Result<Vec<DailyCategory>, String> {
    let categories = categories.unwrap_or_else(|| DailyCategory::ALL.to_vec());
    let unique = categories.iter().copied().collect::<HashSet<_>>();
    if categories.len() > DailyCategory::ALL.len() || unique.len() != categories.len() {
        return Err("清理类别无效".into());
    }
    if mode == ScanMode::Quick && categories.is_empty() {
        return Err("请至少选择一个清理类别".into());
    }
    Ok(categories)
}

#[tauri::command]
fn bootstrap(store: tauri::State<'_, Storage>) -> Result<Bootstrap, String> {
    Ok(Bootstrap {
        disk: scanner::disk()?,
        home: scanner::home()?.display().to_string(),
        settings: store.settings()?,
        history: store.history()?,
    })
}

#[tauri::command]
async fn scan_disk(
    window: tauri::Window,
    state: tauri::State<'_, Arc<State>>,
    store: tauri::State<'_, Storage>,
    mode: ScanMode,
    scan_id: String,
    root: Option<String>,
    daily_categories: Option<Vec<DailyCategory>>,
) -> Result<ScanReport, String> {
    validate_id(&scan_id)?;
    let daily_categories = validate_daily_categories(mode, daily_categories)?;
    let state = Arc::clone(&state);
    let (guard, control) = begin_scan(&state, mode, &scan_id)?;
    let settings = store.settings()?;
    state
        .data
        .lock()
        .map_err(|_| "状态不可用")?
        .snapshots
        .insert(
            mode,
            Snapshot {
                id: scan_id.clone(),
                mode,
                targets: HashMap::new(),
                children: HashMap::new(),
            },
        );
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let snapshot_state = Arc::clone(&state);
        let snapshot_id = scan_id.clone();
        let (report, targets) = scanner::run(
            &scan_id,
            mode,
            (root.as_deref(), &daily_categories),
            &scanner::home()?,
            &settings,
            &control,
            (
                |progress| {
                    let _ = window.emit("scan-progress", progress);
                },
                move |target| {
                    if let Ok(mut data) = snapshot_state.data.lock() {
                        if let Some(snapshot) = data
                            .snapshots
                            .get_mut(&mode)
                            .filter(|snapshot| snapshot.id == snapshot_id)
                        {
                            snapshot.record(target);
                            if mode != ScanMode::Full
                                && snapshot.targets.len() > scanner::MAX_RESULTS
                            {
                                if let Some(remove) = snapshot
                                    .targets
                                    .values()
                                    .min_by(|left, right| {
                                        left.item
                                            .bytes
                                            .cmp(&right.item.bytes)
                                            .then_with(|| right.item.path.cmp(&left.item.path))
                                    })
                                    .map(|target| target.item.id.clone())
                                {
                                    snapshot.targets.remove(&remove);
                                }
                            }
                        }
                    }
                },
            ),
        )?;
        if mode == ScanMode::Full {
            // All final tree nodes have already been published into the active snapshot.
            return Ok(report);
        }
        let mut snapshot = Snapshot {
            id: scan_id,
            mode,
            targets: HashMap::new(),
            children: HashMap::new(),
        };
        for target in targets {
            snapshot.record(target);
        }
        state
            .data
            .lock()
            .map_err(|_| "状态不可用")?
            .snapshots
            .insert(mode, snapshot);
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn cancel_scan(state: tauri::State<'_, Arc<State>>, scan_id: String) -> Result<bool, String> {
    control_scan(&state, &scan_id, "cancel")
}

fn control_scan(state: &State, scan_id: &str, action: &str) -> Result<bool, String> {
    validate_id(scan_id)?;
    let data = state.data.lock().map_err(|_| "状态不可用")?;
    if let Some((_, control)) = data.scans.values().find(|(id, _)| id == scan_id) {
        match action {
            "pause" => control.pause(),
            "resume" => control.resume(),
            "cancel" => control.cancel(),
            _ => return Err("无效任务操作".into()),
        };
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
fn pause_scan(state: tauri::State<'_, Arc<State>>, scan_id: String) -> Result<bool, String> {
    control_scan(&state, &scan_id, "pause")
}

#[tauri::command]
fn resume_scan(state: tauri::State<'_, Arc<State>>, scan_id: String) -> Result<bool, String> {
    control_scan(&state, &scan_id, "resume")
}

#[tauri::command]
fn scan_candidates(
    state: tauri::State<'_, Arc<State>>,
    scan_id: String,
) -> Result<Vec<Candidate>, String> {
    validate_id(&scan_id)?;
    snapshot_items(&state, &scan_id)
}

#[tauri::command]
fn analysis_directory(
    state: tauri::State<'_, Arc<State>>,
    scan_id: String,
    candidate_id: Option<String>,
    prioritize: bool,
) -> Result<Option<AnalysisDirectory>, String> {
    analysis_items(&state, &scan_id, candidate_id.as_deref(), prioritize)
}

fn analysis_items(
    state: &State,
    scan_id: &str,
    candidate_id: Option<&str>,
    prioritize: bool,
) -> Result<Option<AnalysisDirectory>, String> {
    validate_id(scan_id)?;
    if let Some(id) = candidate_id {
        validate_candidate_id(id)?;
    }
    let id = candidate_id.unwrap_or(analysis::ROOT_ID);
    let data = state.data.lock().map_err(|_| "状态不可用")?;
    let snapshot = data
        .snapshots
        .get(&ScanMode::Full)
        .filter(|s| s.id == scan_id)
        .ok_or("扫描已失效")?;
    if snapshot.targets.is_empty() {
        return Ok(None);
    }
    let directory = snapshot
        .targets
        .get(id)
        .filter(|t| t.item.is_dir)
        .ok_or("目录尚未发现")?;
    if let Some((_, control)) = data.scans.get(&ScanMode::Full).filter(|_| prioritize) {
        control.prioritize(id.into());
    }
    let (children, truncated) = analysis_children(snapshot, &directory.item.path);
    Ok(Some(AnalysisDirectory {
        directory: directory.item.clone(),
        children,
        truncated,
    }))
}

fn analysis_children(snapshot: &Snapshot, path: &str) -> (Vec<Candidate>, bool) {
    let mut children: Vec<&Candidate> = snapshot
        .children
        .get(path)
        .into_iter()
        .flatten()
        .filter_map(|id| snapshot.targets.get(id).map(|t| &t.item))
        .collect();
    // ponytail: return the largest 500 children per column; use pagination/virtualization for larger lists.
    let truncated = children.len() > scanner::MAX_RESULTS;
    if truncated {
        children.select_nth_unstable_by(scanner::MAX_RESULTS, |a, b| {
            b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path))
        });
        children.truncate(scanner::MAX_RESULTS);
    }
    children.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    (children.into_iter().cloned().collect(), truncated)
}

#[tauri::command]
fn save_settings(
    state: tauri::State<'_, Arc<State>>,
    store: tauri::State<'_, Storage>,
    settings: Settings,
) -> Result<SettingsUpdate, String> {
    let _guard = begin(&state, "settings")?;
    if !state
        .data
        .lock()
        .map_err(|_| "状态不可用")?
        .scans
        .is_empty()
    {
        return Err("请结束或取消所有扫描后修改设置（暂停任务仍在扫描中）".into());
    }
    persist_settings(&state, &store, settings)
}

fn persist_settings(
    state: &State,
    store: &Storage,
    settings: Settings,
) -> Result<SettingsUpdate, String> {
    let settings = store.save_settings(&settings, &scanner::home()?)?;
    let mut data = state.data.lock().map_err(|_| "状态不可用")?;
    data.pending = None;
    for snapshot in data.snapshots.values_mut() {
        for target in snapshot.targets.values_mut() {
            apply_settings_to_target(target, &settings);
        }
    }
    Ok(SettingsUpdate { settings })
}

fn apply_settings_to_target(target: &mut Target, settings: &Settings) {
    let path = Path::new(&target.item.path);
    let excluded_by = settings
        .excluded_paths
        .iter()
        .find(|rule| path.starts_with(rule) || Path::new(rule).starts_with(path))
        .cloned();
    if let Some(rule) = excluded_by {
        target.item.cleanable = false;
        target.item.recommended = false;
        target.item.status = "manual".into();
        target.item.reason = "已加入保护名单".into();
        target.item.protection = Some("manual".into());
        target.item.excluded_by = Some(rule);
    } else if target.item.protection.as_deref() == Some("manual") {
        target.item.cleanable = false;
        target.item.recommended = false;
        target.item.status = "unknown".into();
        target.item.reason = "保护规则已更新，请重新检查此项".into();
        target.item.protection = Some("unknown".into());
        target.item.excluded_by = None;
    }
}

fn choose_targets(snapshot: &Snapshot, ids: &[String]) -> Result<Vec<Target>, String> {
    if ids.is_empty() {
        return Err("请选择要清理的项目".into());
    }
    if ids.len() > scanner::MAX_RESULTS {
        return Err("一次最多选择 500 个项目".into());
    }
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for id in ids {
        validate_candidate_id(id)?;
        if !seen.insert(id) {
            continue;
        }
        let target = snapshot.targets.get(id).ok_or("候选项不存在，请重新扫描")?;
        if snapshot.mode == ScanMode::Full && id == analysis::ROOT_ID {
            return Err("请选择目录内的项目".into());
        }
        if !target.item.cleanable || !target.item.complete {
            return Err(format!("{}：{}", target.item.title, target.item.reason));
        }
        if target.identity.is_none() {
            return Err("缺少路径身份".into());
        }
        targets.push(target.clone());
    }
    for (index, target) in targets.iter().enumerate() {
        let path = &target.identity.as_ref().unwrap().path;
        if targets.iter().skip(index + 1).any(|other| {
            let other = &other.identity.as_ref().unwrap().path;
            path.starts_with(other) || other.starts_with(path)
        }) {
            return Err("所选路径有重叠，请分别清理".into());
        }
    }
    Ok(targets)
}

fn remove_cleaned_targets(snapshot: &mut Snapshot, outcomes: &[ItemOutcome]) {
    let removed_paths = outcomes
        .iter()
        .filter(|item| matches!(item.status.as_str(), "removed" | "trashed"))
        .map(|item| item.path.as_str())
        .collect::<HashSet<_>>();
    let removed = snapshot
        .targets
        .values()
        .filter(|target| removed_paths.contains(target.item.path.as_str()))
        .map(|target| target.item.clone())
        .collect::<Vec<_>>();
    for target in snapshot.targets.values_mut() {
        for item in &removed {
            if Path::new(&item.path).starts_with(&target.item.path) && item.path != target.item.path
            {
                target.item.bytes = target.item.bytes.saturating_sub(item.bytes);
                target.item.entries = target.item.entries.saturating_sub(item.entries);
            }
        }
    }
    snapshot.targets.retain(|_, target| {
        !removed_paths
            .iter()
            .any(|path| Path::new(&target.item.path).starts_with(path))
    });
    snapshot.children.retain(|path, ids| {
        ids.retain(|id| snapshot.targets.contains_key(id));
        !removed_paths
            .iter()
            .any(|removed| Path::new(path).starts_with(removed))
    });
}

fn choose_snapshot_targets(
    state: &State,
    scan_id: &str,
    ids: &[String],
) -> Result<Vec<Target>, String> {
    let data = state.data.lock().map_err(|_| "状态不可用")?;
    let snapshot = data
        .snapshots
        .values()
        .find(|snapshot| snapshot.id == scan_id)
        .ok_or("扫描已失效")?;
    if data.scans.contains_key(&snapshot.mode) {
        return Err("扫描仍在运行，请先暂停并结束剩余扫描后再预览清理".into());
    }
    choose_targets(snapshot, ids)
}

fn snapshot_items(state: &State, scan_id: &str) -> Result<Vec<Candidate>, String> {
    let data = state.data.lock().map_err(|_| "状态不可用")?;
    let snapshot = data
        .snapshots
        .values()
        .find(|snapshot| snapshot.id == scan_id)
        .ok_or("扫描已失效")?;
    if snapshot.mode == ScanMode::Full {
        let root = snapshot
            .targets
            .get(analysis::ROOT_ID)
            .ok_or("目录尚未发现")?;
        return Ok(analysis_children(snapshot, &root.item.path).0);
    }
    Ok(snapshot
        .targets
        .values()
        .map(|target| target.item.clone())
        .collect())
}

fn validate_in_parallel<T, C, P>(
    items: &[T],
    control: &ScanControl,
    check: C,
    mut progress: P,
) -> Result<(), String>
where
    T: Sync,
    C: Fn(&T) -> Result<(), String> + Sync,
    P: FnMut(usize, usize, usize),
{
    if items.is_empty() {
        return Ok(());
    }
    let next = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel();
    let workers = items.len().min(4);
    let mut completed = 0;
    let mut first_error: Option<(usize, String)> = None;

    thread::scope(|scope| {
        for _ in 0..workers {
            let sender = sender.clone();
            let next = &next;
            let abort = &abort;
            let check = &check;
            scope.spawn(move || loop {
                if abort.load(Ordering::Relaxed) || !control.checkpoint() {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break;
                };
                let result = check(item);
                if result.is_err() {
                    abort.store(true, Ordering::Relaxed);
                }
                if sender.send((index, result)).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        while let Ok((index, result)) = receiver.recv() {
            completed += 1;
            progress(index, completed, items.len());
            if let Err(error) = result {
                if first_error
                    .as_ref()
                    .is_none_or(|(existing, _)| index < *existing)
                {
                    first_error = Some((index, error));
                }
            }
        }
    });

    if control.cancelled() {
        return Err("预览校验已取消".into());
    }
    if let Some((_, error)) = first_error {
        return Err(error);
    }
    if completed != items.len() {
        return Err("预览校验未完整完成".into());
    }
    Ok(())
}

#[tauri::command]
async fn recheck_item(
    state: tauri::State<'_, Arc<State>>,
    store: tauri::State<'_, Storage>,
    scan_id: String,
    candidate_id: String,
    remove_manual_protection: bool,
) -> Result<Candidate, String> {
    validate_id(&scan_id)?;
    validate_candidate_id(&candidate_id)?;
    let state = Arc::clone(&state);
    let guard = begin(&state, "recheck")?;
    let (mode, target) = {
        let mut data = state.data.lock().map_err(|_| "状态不可用")?;
        data.pending = None;
        let snapshot = data
            .snapshots
            .values()
            .find(|s| s.id == scan_id)
            .ok_or("扫描已失效")?;
        (
            snapshot.mode,
            snapshot
                .targets
                .get(&candidate_id)
                .ok_or("候选不存在")?
                .clone(),
        )
    };
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let home = scanner::home()?;
        let mut settings = store.settings()?;
        if remove_manual_protection {
            // Only remove a stored user rule; never turn off mounted/busy/system gates.
            let rule = target
                .item
                .excluded_by
                .as_ref()
                .ok_or("此项没有手动保护规则，不能强制解锁")?;
            settings.excluded_paths.retain(|path| path != rule);
        }
        let updated = scanner::recheck(&target, mode, &home, &settings)?;
        if remove_manual_protection {
            store.save_settings(&settings, &home)?;
        }
        let mut data = state.data.lock().map_err(|_| "状态不可用")?;
        let snapshot = data
            .snapshots
            .get_mut(&mode)
            .filter(|s| s.id == scan_id)
            .ok_or("扫描已失效")?;
        snapshot.targets.insert(candidate_id, updated.clone());
        data.pending = None;
        Ok(updated.item)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn prepare_cleanup(
    window: tauri::Window,
    state: tauri::State<'_, Arc<State>>,
    store: tauri::State<'_, Storage>,
    scan_id: String,
    candidate_ids: Vec<String>,
) -> Result<CleanupPreview, String> {
    validate_id(&scan_id)?;
    let state = Arc::clone(&state);
    let preparation_id = format!("prepare-{scan_id}");
    let (guard, control) = begin_preparation(&state, &preparation_id)?;
    state.data.lock().map_err(|_| "状态不可用")?.pending = None;
    let targets = choose_snapshot_targets(&state, &scan_id, &candidate_ids)?;
    let trash = state
        .data
        .lock()
        .map_err(|_| "状态不可用")?
        .snapshots
        .values()
        .any(|snapshot| snapshot.id == scan_id && snapshot.mode == ScanMode::Full);
    let settings = store.settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let home = scanner::home()?;
        let total = targets.len();
        validate_in_parallel(
            &targets,
            &control,
            |target| {
                if trash {
                    scanner::analysis_precheck(target, &home, &settings)
                } else {
                    scanner::cleanup_precheck(target, &home, &settings)
                }
            },
            |index, completed, _| {
                let _ = window.emit(
                    "cleanup-validation-progress",
                    CleanupValidationProgress {
                        scan_id: scan_id.clone(),
                        completed,
                        total,
                        current_path: targets[index].item.path.clone(),
                    },
                );
            },
        )?;
        if !control.checkpoint() {
            return Err("预览校验已取消".into());
        }
        let _ = window.emit(
            "cleanup-validation-progress",
            CleanupValidationProgress {
                scan_id: scan_id.clone(),
                completed: total,
                total,
                current_path: String::new(),
            },
        );
        let token = format!(
            "cleanup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        let preview = CleanupPreview {
            token: token.clone(),
            trash,
            estimated_bytes: targets.iter().map(|t| t.item.bytes).sum(),
            items: targets.iter().map(|t| t.item.clone()).collect(),
        };
        let mut data = state.data.lock().map_err(|_| "状态不可用")?;
        if control.cancelled() {
            return Err("预览校验已取消".into());
        }
        data.pending = Some(Plan {
            token,
            scan_id,
            trash,
            targets,
            created: Instant::now(),
        });
        Ok(preview)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn cancel_prepare_cleanup(
    state: tauri::State<'_, Arc<State>>,
    scan_id: String,
) -> Result<bool, String> {
    validate_id(&scan_id)?;
    cancel_preparation(&state, &scan_id)
}

fn cancel_preparation(state: &State, scan_id: &str) -> Result<bool, String> {
    let mut data = state.data.lock().map_err(|_| "状态不可用")?;
    if let Some(preparation) = &data.preparation {
        if preparation.id == format!("prepare-{scan_id}") {
            preparation.control.cancel();
            if data
                .pending
                .as_ref()
                .is_some_and(|plan| plan.scan_id == scan_id)
            {
                data.pending = None;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

// Targets are disjoint (choose_targets). Keep audit writes serialized, while each
// worker refreshes safety evidence immediately before removing its own target.
fn cleanup_in_parallel<C, R, S>(history: &mut HistoryEntry, check: C, remove: R, record: S)
where
    C: Fn(usize) -> Result<(), String> + Sync,
    R: Fn(usize) -> Result<u64, String> + Sync,
    S: Fn(&HistoryEntry) -> Result<(), String> + Sync,
{
    let count = history.items.len();
    let shared = Mutex::new(history);
    let next = AtomicUsize::new(0);
    // ponytail: cap filesystem/probe contention at four targets; tune only with workload measurements.
    thread::scope(|scope| {
        for _ in 0..count.min(4) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= count {
                    break;
                }
                {
                    let mut history = shared.lock().unwrap();
                    if history.status != "running" {
                        break;
                    }
                    history.items[index].status = "running".into();
                    history.items[index].message = "正在执行最终安全检查；尚未删除".into();
                    if let Err(error) = record(&history) {
                        history.status = "partial".into();
                        history.items[index].status = "skipped".into();
                        history.items[index].message = format!("记录保存失败，已停止：{error}");
                        break;
                    }
                }
                let eligibility = check(index);
                let outcome = eligibility.and_then(|_| {
                    {
                        let mut history = shared.lock().unwrap();
                        if history.status != "running" {
                            return Err("记录保存失败，已停止；尚未删除".into());
                        }
                        history.items[index].message =
                            "正在处理；若应用中断，请检查路径现状".into();
                        if let Err(error) = record(&history) {
                            history.status = "partial".into();
                            history.items[index].message = "正在执行最终安全检查；尚未删除".into();
                            return Err(format!("记录保存失败，已停止：{error}"));
                        }
                    }
                    remove(index)
                });
                let mut history = shared.lock().unwrap();
                match outcome {
                    Ok(bytes) => {
                        history.items[index].status = "removed".into();
                        history.items[index].message = "已永久清理".into();
                        history.estimated_bytes = history.estimated_bytes.saturating_add(bytes);
                    }
                    Err(error) => {
                        history.items[index].status = if history.items[index].message
                            == "正在执行最终安全检查；尚未删除"
                        {
                            "skipped"
                        } else {
                            "failed"
                        }
                        .into();
                        history.items[index].message = error;
                    }
                }
                if let Err(error) = record(&history) {
                    history.status = "partial".into();
                    history.items[index]
                        .message
                        .push_str(&format!("；记录保存失败，已停止：{error}"));
                }
            });
        }
    });
}

#[tauri::command]
async fn execute_cleanup(
    state: tauri::State<'_, Arc<State>>,
    store: tauri::State<'_, Storage>,
    token: String,
) -> Result<HistoryEntry, String> {
    validate_id(&token)?;
    let state = Arc::clone(&state);
    let guard = begin(&state, &token)?;
    let plan = {
        let mut data = state.data.lock().map_err(|_| "状态不可用")?;
        if data.pending.as_ref().is_none_or(|p| p.token != token) {
            return Err("确认已失效，请重新预览".into());
        }
        data.pending.take().ok_or("确认已失效")?
    };
    if plan.created.elapsed() > Duration::from_secs(120) {
        return Err("确认超过 2 分钟，请重新预览".into());
    }
    {
        let data = state.data.lock().map_err(|_| "状态不可用")?;
        if !data.snapshots.values().any(|s| s.id == plan.scan_id) {
            return Err("扫描已失效".into());
        }
    }
    let store = store.inner().clone();
    let settings = store.settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let home = scanner::home()?;
        let mut history = HistoryEntry {
            id: plan.token.clone(),
            started_at: scanner::now(),
            status: "running".into(),
            estimated_bytes: 0,
            available_before: scanner::disk()?.available_bytes,
            available_after: None,
            items: plan
                .targets
                .iter()
                .map(|t| ItemOutcome {
                    title: t.item.title.clone(),
                    path: t.item.path.clone(),
                    status: "pending".into(),
                    message: String::new(),
                })
                .collect(),
        };
        store.record(&history)?; // Fail before deleting if the audit record cannot be persisted.
        cleanup_in_parallel(
            &mut history,
            |index| {
                if plan.trash {
                    scanner::analysis_check(&plan.targets[index], &home, &settings)
                } else {
                    scanner::cleanup_check(&plan.targets[index], &home, &settings)
                }
            },
            |index| {
                let target = &plan.targets[index];
                let identity = target.identity.as_ref().ok_or("缺少路径身份")?;
                if plan.trash {
                    safety::trash(identity)?;
                } else {
                    safety::remove(identity, &format!("{}-{index}", plan.token))?;
                }
                Ok(target.item.bytes)
            },
            |history| {
                if plan.trash {
                    let mut entry = history.clone();
                    mark_trashed(&mut entry);
                    store.record(&entry)
                } else {
                    store.record(history)
                }
            },
        );
        if history.status == "running" {
            history.status = if history.items.iter().all(|i| i.status == "removed") {
                "complete"
            } else {
                "partial"
            }
            .into();
        }
        if plan.trash {
            mark_trashed(&mut history);
        }
        history.available_after = scanner::disk().ok().map(|d| d.available_bytes);
        if let Err(error) = store.record(&history) {
            history.status = "partial".into();
            if let Some(item) = history.items.last_mut() {
                item.message
                    .push_str(&format!("；最终记录保存失败：{error}"));
            }
        }
        // Keep the scan usable after cleanup. Only successfully removed targets
        // leave the snapshot; skipped and failed targets remain available for review.
        let mut data = state.data.lock().map_err(|_| "状态不可用")?;
        if let Some(snapshot) = data.snapshots.values_mut().find(|s| s.id == plan.scan_id) {
            remove_cleaned_targets(snapshot, &history.items);
        }
        Ok(history)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn mark_trashed(history: &mut HistoryEntry) {
    for item in &mut history.items {
        if item.status == "removed" {
            item.status = "trashed".into();
            item.message = item.message.replacen(
                "已永久清理",
                "已移到废纸篓，可在 Finder 中恢复；空间尚未释放",
                1,
            );
        }
    }
}

#[tauri::command]
async fn reveal_item(
    state: tauri::State<'_, Arc<State>>,
    scan_id: String,
    candidate_id: String,
) -> Result<(), String> {
    validate_id(&scan_id)?;
    validate_candidate_id(&candidate_id)?;
    let path = {
        let data = state.data.lock().map_err(|_| "状态不可用")?;
        let snapshot = data
            .snapshots
            .values()
            .find(|s| s.id == scan_id)
            .ok_or("扫描已失效")?;
        snapshot
            .targets
            .get(&candidate_id)
            .ok_or("候选不存在")?
            .item
            .path
            .clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        let (ok, _) = safety::command_output("/usr/bin/open", &["-R", &path], None)?;
        if ok {
            Ok(())
        } else {
            Err("Finder 无法定位，请确认路径仍存在".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .manage(Arc::new(State::default()))
        .setup(|app| {
            let data = app.path().app_data_dir()?;
            app.manage(Storage(data));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            scan_disk,
            cancel_scan,
            pause_scan,
            resume_scan,
            scan_candidates,
            analysis_directory,
            recheck_item,
            save_settings,
            prepare_cleanup,
            cancel_prepare_cleanup,
            execute_cleanup,
            reveal_item
        ])
        .run(tauri::generate_context!())
        .expect("无法启动 CDisk");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_analysis_can_browse_prioritize_and_reuse_a_single_tree() {
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        for name in ["a", "b"] {
            std::fs::create_dir(home.join(name)).unwrap();
            std::fs::write(home.join(name).join("file"), b"cached bytes").unwrap();
        }
        let state = Arc::new(State::default());
        let (guard, control) = begin_scan(&state, ScanMode::Full, "live-tree").unwrap();
        state.data.lock().unwrap().snapshots.insert(
            ScanMode::Full,
            Snapshot {
                id: "live-tree".into(),
                mode: ScanMode::Full,
                targets: HashMap::new(),
                children: HashMap::new(),
            },
        );
        let (ready, wait) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_control = Arc::clone(&control);
        let worker_home = home.clone();
        let worker = thread::spawn(move || {
            let _guard = guard;
            let paused = AtomicBool::new(false);
            scanner::run(
                "live-tree",
                ScanMode::Full,
                (worker_home.to_str(), &[]),
                &worker_home,
                &Settings::default(),
                &worker_control,
                (
                    |_| {
                        if worker_state.data.lock().unwrap().snapshots[&ScanMode::Full]
                            .targets
                            .len()
                            >= 3
                            && !paused.swap(true, Ordering::Relaxed)
                        {
                            worker_control.pause();
                            ready.send(()).unwrap();
                        }
                    },
                    |target| {
                        worker_state
                            .data
                            .lock()
                            .unwrap()
                            .snapshots
                            .get_mut(&ScanMode::Full)
                            .unwrap()
                            .record(target);
                    },
                ),
            )
            .unwrap()
        });
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        let root = analysis_items(&state, "live-tree", None, false)
            .unwrap()
            .unwrap();
        assert_eq!(root.children.len(), 2);
        assert!(root
            .children
            .iter()
            .all(|item| !item.complete && !item.cleanable));
        let priority = root
            .children
            .iter()
            .max_by_key(|item| item.id.clone())
            .unwrap();
        let directory = analysis_items(&state, "live-tree", Some(&priority.id), true)
            .unwrap()
            .unwrap();
        assert!(directory.children.is_empty());
        assert!(analysis_items(&state, "different-tree", None, false).is_err());
        assert!(analysis_items(&state, "live-tree", Some("unknown"), false).is_err());
        control.resume();
        let (_, targets) = worker.join().unwrap();
        let first_file = targets
            .iter()
            .find(|target| target.item.id == "item-3")
            .unwrap();
        assert_eq!(
            Path::new(&first_file.item.path).parent(),
            Some(Path::new(&priority.path)),
            "the selected directory is enumerated first"
        );
        let directory = analysis_items(&state, "live-tree", Some(&priority.id), false)
            .unwrap()
            .unwrap();
        assert!(directory.directory.complete);
        assert_eq!(directory.children.len(), 1);
        let file = &directory.children[0];
        std::fs::remove_file(&file.path).unwrap();
        let cached = analysis_items(&state, "live-tree", Some(&priority.id), false)
            .unwrap()
            .unwrap();
        assert_eq!(
            cached.children[0].bytes, file.bytes,
            "navigation reads only the retained tree"
        );
        assert!(choose_snapshot_targets(&state, "live-tree", &[analysis::ROOT_ID.into()]).is_err());
        let mut data = state.data.lock().unwrap();
        let snapshot = data.snapshots.get_mut(&ScanMode::Full).unwrap();
        let before = snapshot.targets[analysis::ROOT_ID].item.bytes;
        remove_cleaned_targets(
            snapshot,
            &[ItemOutcome {
                title: priority.title.clone(),
                path: priority.path.clone(),
                status: "trashed".into(),
                message: String::new(),
            }],
        );
        assert!(
            !snapshot.targets.contains_key(&file.id),
            "removing a directory also drops descendants"
        );
        assert_eq!(
            analysis_children(snapshot, &home.display().to_string())
                .0
                .len(),
            1
        );
        assert_eq!(
            snapshot.targets[analysis::ROOT_ID].item.bytes,
            before.saturating_sub(directory.directory.bytes)
        );
    }

    #[test]
    fn analysis_columns_bound_payloads_without_discarding_tree_nodes() {
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        for index in 0..=scanner::MAX_RESULTS {
            std::fs::write(home.join(format!("file-{index:04}")), b"fixture").unwrap();
        }
        let (report, targets) = scanner::run(
            "large-tree",
            ScanMode::Full,
            (home.to_str(), &[]),
            &home,
            &Settings::default(),
            &ScanControl::default(),
            (|_| {}, |_| {}),
        )
        .unwrap();
        assert!(report.truncated);
        let mut snapshot = Snapshot {
            id: "large-tree".into(),
            mode: ScanMode::Full,
            targets: HashMap::new(),
            children: HashMap::new(),
        };
        for target in targets {
            snapshot.record(target);
        }
        assert_eq!(snapshot.targets.len(), scanner::MAX_RESULTS + 2);
        let (children, truncated) = analysis_children(&snapshot, &home.display().to_string());
        assert!(truncated);
        assert_eq!(children.len(), scanner::MAX_RESULTS);
        assert!(children.iter().all(|item| item.complete));
    }

    fn cleanup_history(count: usize) -> HistoryEntry {
        HistoryEntry {
            id: "cleanup-test".into(),
            started_at: 0,
            status: "running".into(),
            estimated_bytes: 0,
            available_before: 0,
            available_after: None,
            items: (0..count)
                .map(|index| ItemOutcome {
                    title: index.to_string(),
                    path: index.to_string(),
                    status: "pending".into(),
                    message: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn trash_history_keeps_failures_and_audit_diagnostics() {
        let mut history = cleanup_history(3);
        history.items[0].status = "removed".into();
        history.items[0].message = "已永久清理；记录保存失败，已停止：disk full".into();
        history.items[1].status = "skipped".into();
        history.items[1].message = "busy".into();
        history.items[2].status = "failed".into();
        mark_trashed(&mut history);
        assert_eq!(history.items[0].status, "trashed");
        assert!(history.items[0].message.contains("空间尚未释放"));
        assert!(history.items[0].message.contains("disk full"));
        assert_eq!(history.items[1].status, "skipped");
        assert_eq!(history.items[1].message, "busy");
        assert_eq!(history.items[2].status, "failed");
    }

    #[test]
    fn cleanup_overlaps_four_targets_and_preserves_outcomes() {
        let mut history = cleanup_history(12);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(4);
        let removed = Mutex::new(Vec::new());
        cleanup_in_parallel(
            &mut history,
            |index| {
                let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(running, Ordering::SeqCst);
                if index < 4 {
                    barrier.wait();
                }
                thread::sleep(Duration::from_millis(5));
                if index == 5 {
                    active.fetch_sub(1, Ordering::SeqCst);
                    return Err("busy".into());
                }
                Ok(())
            },
            |index| {
                removed.lock().unwrap().push(index);
                active.fetch_sub(1, Ordering::SeqCst);
                if index == 7 {
                    return Err("delete failed".into());
                }
                Ok(10)
            },
            |_| Ok(()),
        );
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(history.estimated_bytes, 100);
        assert_eq!(history.items[5].status, "skipped");
        assert_eq!(history.items[7].status, "failed");
        assert_eq!(
            history
                .items
                .iter()
                .filter(|i| i.status == "removed")
                .count(),
            10
        );
        let mut removed = removed.into_inner().unwrap();
        removed.sort_unstable();
        assert_eq!(removed, (0..12).filter(|i| *i != 5).collect::<Vec<_>>());
    }

    #[test]
    fn cleanup_audit_failure_prevents_unrecorded_deletion() {
        for fail_at in [0, 1] {
            let mut history = cleanup_history(1);
            let writes = AtomicUsize::new(0);
            cleanup_in_parallel(
                &mut history,
                |_| Ok(()),
                |_| panic!("must persist the deletion intent before removing"),
                |_| {
                    if writes.fetch_add(1, Ordering::SeqCst) == fail_at {
                        Err("disk full".into())
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(history.status, "partial");
            assert_eq!(history.items[0].status, "skipped");
        }
    }

    #[test]
    fn cleanup_audit_failure_stops_other_validated_workers() {
        let mut history = cleanup_history(12);
        let barrier = std::sync::Barrier::new(4);
        let checks = AtomicUsize::new(0);
        cleanup_in_parallel(
            &mut history,
            |_| {
                checks.fetch_add(1, Ordering::SeqCst);
                barrier.wait();
                Ok(())
            },
            |_| panic!("audit failed before any deletion"),
            |history| {
                if history
                    .items
                    .iter()
                    .any(|i| i.message.starts_with("正在处理"))
                {
                    Err("disk full".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(checks.load(Ordering::SeqCst), 4);
        assert_eq!(history.status, "partial");
        assert_eq!(
            history
                .items
                .iter()
                .filter(|i| i.status == "skipped")
                .count(),
            4
        );
        assert_eq!(
            history
                .items
                .iter()
                .filter(|i| i.status == "pending")
                .count(),
            8
        );
    }

    #[test]
    fn cleanup_removes_disjoint_temporary_directories_with_persisted_audit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let store = Storage(root.join("audit"));
        let identities = (0..8)
            .map(|index| {
                let path = root.join(index.to_string());
                std::fs::create_dir(&path).unwrap();
                std::fs::write(path.join("data"), b"test").unwrap();
                safety::capture(&path).unwrap()
            })
            .collect::<Vec<_>>();
        let mut history = cleanup_history(identities.len());
        store.record(&history).unwrap();
        cleanup_in_parallel(
            &mut history,
            |index| safety::verify(&identities[index]),
            |index| {
                safety::remove(&identities[index], &format!("test-{index}"))?;
                Ok(4)
            },
            |history| store.record(history),
        );
        assert!(identities.iter().all(|identity| !identity.path.exists()));
        assert_eq!(history.estimated_bytes, 32);
        let saved = store.history().unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved[0].items.iter().all(|item| item.status == "removed"));
    }

    #[test]
    #[ignore = "manual cleanup benchmark; creates and deletes only temporary fixtures"]
    fn benchmark_cleanup_batch() {
        for parallel in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let store = Storage(root.join("audit"));
            let identities = (0..24)
                .map(|index| {
                    let path = root.join(index.to_string());
                    std::fs::create_dir(&path).unwrap();
                    for file in 0..100 {
                        std::fs::write(path.join(file.to_string()), b"test").unwrap();
                    }
                    safety::capture(&path).unwrap()
                })
                .collect::<Vec<_>>();
            let check = |index: usize| {
                safety::mount_paths()?;
                safety::process_table()?;
                safety::verify(&identities[index])?;
                safety::path_idle(&identities[index].path).map_err(|error| error.reason)
            };
            let remove = |index: usize| {
                safety::remove(&identities[index], &format!("bench-{index}"))?;
                Ok(400)
            };
            let mut history = cleanup_history(identities.len());
            let start = Instant::now();
            store.record(&history).unwrap();
            if parallel {
                cleanup_in_parallel(&mut history, check, remove, |history| store.record(history));
            } else {
                for index in 0..identities.len() {
                    history.items[index].status = "running".into();
                    store.record(&history).unwrap();
                    check(index).unwrap();
                    history.items[index].message = "正在处理；若应用中断，请检查路径现状".into();
                    store.record(&history).unwrap();
                    history.estimated_bytes += remove(index).unwrap();
                    history.items[index].status = "removed".into();
                    store.record(&history).unwrap();
                }
            }
            assert!(history.items.iter().all(|item| item.status == "removed"));
            eprintln!(
                "cleanup parallel={parallel}: {:?} (24 directories, 2400 files)",
                start.elapsed()
            );
        }
    }

    #[test]
    fn single_operation_guard_releases_on_drop() {
        let state = Arc::new(State::default());
        let guard = begin(&state, "mutation").unwrap();
        assert!(begin(&state, "another").is_err());
        drop(guard);
        assert!(begin(&state, "another").is_ok());
    }

    #[test]
    fn preparation_can_be_cancelled_and_releases_mutation_guard() {
        let state = Arc::new(State::default());
        let (guard, control) = begin_preparation(&state, "prepare-scan").unwrap();
        assert!(state.data.lock().unwrap().mutation);
        assert_eq!(
            state.data.lock().unwrap().preparation.as_ref().unwrap().id,
            "prepare-scan"
        );
        state.data.lock().unwrap().pending = Some(Plan {
            token: "stale".into(),
            scan_id: "scan".into(),
            trash: false,
            targets: vec![],
            created: Instant::now(),
        });
        assert!(cancel_preparation(&state, "scan").unwrap());
        assert!(control.cancelled());
        assert!(state.data.lock().unwrap().pending.is_none());
        drop(guard);
        let data = state.data.lock().unwrap();
        assert!(!data.mutation);
        assert!(data.preparation.is_none());
    }

    #[test]
    fn batch_validation_is_bounded_reports_progress_and_stops_after_failure() {
        let control = ScanControl::default();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let checked = AtomicUsize::new(0);
        let mut progress = Vec::new();
        let items = (0..231).collect::<Vec<_>>();
        let error = validate_in_parallel(
            &items,
            &control,
            |item| {
                let active_now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(active_now, Ordering::SeqCst);
                checked.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(1));
                active.fetch_sub(1, Ordering::SeqCst);
                if *item == 37 {
                    Err("fixture failure".into())
                } else {
                    Ok(())
                }
            },
            |_, completed, total| progress.push((completed, total)),
        )
        .unwrap_err();
        assert_eq!(error, "fixture failure");
        assert!(peak.load(Ordering::SeqCst) <= 4);
        assert!(checked.load(Ordering::SeqCst) < items.len());
        assert_eq!(progress.last().unwrap().1, 231);

        let cancelled = ScanControl::default();
        cancelled.cancel();
        assert_eq!(
            validate_in_parallel(&items, &cancelled, |_| Ok(()), |_, _, _| {}).unwrap_err(),
            "预览校验已取消"
        );
    }

    #[test]
    fn batch_preview_precheck_handles_231_targets_without_deleting() {
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let settings = Settings::default();
        let targets = (0..231)
            .map(|index| {
                let path = home.join(format!("cache-{index}"));
                std::fs::create_dir(&path).unwrap();
                std::fs::write(path.join("data"), b"cache").unwrap();
                Target {
                    item: Candidate {
                        id: format!("item-{index}"),
                        title: format!("cache-{index}"),
                        path: path.display().to_string(),
                        category: "开发缓存".into(),
                        description: String::new(),
                        bytes: 5,
                        entries: 2,
                        modified_at: None,
                        is_dir: true,
                        cleanable: true,
                        recommended: true,
                        status: "ready".into(),
                        reason: "fixture".into(),
                        complete: true,
                        protection: None,
                        excluded_by: None,
                    },
                    identity: Some(safety::capture(&path).unwrap()),
                    owner_patterns: vec![],
                }
            })
            .collect::<Vec<_>>();
        let control = ScanControl::default();
        let mut completed = 0;
        let start = Instant::now();
        validate_in_parallel(
            &targets,
            &control,
            |target| scanner::cleanup_precheck(target, &home, &settings),
            |_, current, total| {
                completed = current;
                assert_eq!(total, 231);
            },
        )
        .unwrap();
        assert_eq!(completed, 231);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "batch precheck took {:?}",
            start.elapsed()
        );
        assert!(targets
            .iter()
            .all(|target| target.identity.as_ref().unwrap().path.exists()));
    }

    #[test]
    fn separate_pages_scan_concurrently_and_controls_are_scoped() {
        let state = Arc::new(State::default());
        let (quick_guard, quick) = begin_scan(&state, ScanMode::Quick, "scan-quick").unwrap();
        let (project_guard, project) =
            begin_scan(&state, ScanMode::Projects, "scan-project").unwrap();
        assert!(begin_scan(&state, ScanMode::Quick, "duplicate").is_err());
        assert!(begin_scan(&state, ScanMode::Full, "scan-quick").is_err());
        assert!(control_scan(&state, "scan-quick", "pause").unwrap());
        assert!(project.checkpoint());
        assert!(control_scan(&state, "scan-quick", "cancel").unwrap());
        assert!(quick.cancelled());
        assert!(!project.cancelled());
        assert!(!control_scan(&state, "stale", "cancel").unwrap());
        drop(quick_guard);
        assert_eq!(state.data.lock().unwrap().scans.len(), 1);
        drop(project_guard);
        assert!(state.data.lock().unwrap().scans.is_empty());
    }

    #[test]
    fn another_pages_scan_does_not_evict_snapshot_or_plan_and_mutations_remain_serial() {
        let state = Arc::new(State::default());
        {
            let mut data = state.data.lock().unwrap();
            data.snapshots.insert(
                ScanMode::Quick,
                Snapshot {
                    id: "quick-result".into(),
                    mode: ScanMode::Quick,
                    targets: HashMap::new(),
                    children: HashMap::new(),
                },
            );
            data.pending = Some(Plan {
                token: "plan".into(),
                scan_id: "quick-result".into(),
                trash: false,
                targets: vec![],
                created: Instant::now(),
            });
        }
        let (_project, _) = begin_scan(&state, ScanMode::Projects, "project-scan").unwrap();
        assert!(state
            .data
            .lock()
            .unwrap()
            .snapshots
            .contains_key(&ScanMode::Quick));
        assert!(state.data.lock().unwrap().pending.is_some());
        let mutation = begin(&state, "cleanup").unwrap();
        assert!(begin(&state, "settings").is_err());
        assert!(begin_scan(&state, ScanMode::Full, "full").is_err());
        drop(mutation);
        let (_quick, _) = begin_scan(&state, ScanMode::Quick, "new-quick").unwrap();
        assert!(state.data.lock().unwrap().pending.is_none());
    }

    #[test]
    fn ids_and_empty_or_unknown_selections_fail_closed() {
        assert!(validate_id("../anything").is_err());
        assert!(validate_id("").is_err());
        assert!(validate_id("scan-123").is_ok());
        assert!(validate_candidate_id("../item").is_err());
        assert!(validate_candidate_id(&"a".repeat(129)).is_err());
        assert!(validate_daily_categories(ScanMode::Quick, Some(vec![])).is_err());
        assert!(validate_daily_categories(
            ScanMode::Quick,
            Some(vec![DailyCategory::User, DailyCategory::User])
        )
        .is_err());
        assert_eq!(
            validate_daily_categories(ScanMode::Quick, None).unwrap(),
            DailyCategory::ALL
        );
        let snapshot = Snapshot {
            id: "scan".into(),
            mode: ScanMode::Quick,
            targets: HashMap::new(),
            children: HashMap::new(),
        };
        assert!(choose_targets(&snapshot, &[]).is_err());
        assert!(choose_targets(&snapshot, &["unknown".into()]).is_err());
        assert!(choose_targets(
            &snapshot,
            &(0..=scanner::MAX_RESULTS)
                .map(|index| format!("item-{index}"))
                .collect::<Vec<_>>()
        )
        .is_err());
    }

    #[test]
    fn selection_deduplicates_ids_and_rejects_overlap_and_read_only_targets() {
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let first = home.join("cache");
        let nested = first.join("child");
        std::fs::create_dir_all(&nested).unwrap();
        let target = |id: &str, path: &std::path::Path| Target {
            item: Candidate {
                id: id.into(),
                title: id.into(),
                path: path.display().to_string(),
                category: "开发缓存".into(),
                description: String::new(),
                bytes: 1,
                entries: 1,
                modified_at: None,
                is_dir: true,
                cleanable: true,
                recommended: false,
                status: "ready".into(),
                reason: "test".into(),
                complete: true,
                protection: None,
                excluded_by: None,
            },
            identity: Some(safety::capture(path).unwrap()),
            owner_patterns: vec![],
        };
        let mut snapshot = Snapshot {
            id: "test".into(),
            mode: ScanMode::Quick,
            children: HashMap::new(),
            targets: HashMap::from([
                ("a".into(), target("a", &first)),
                ("b".into(), target("b", &nested)),
            ]),
        };
        assert_eq!(
            choose_targets(&snapshot, &["a".into(), "a".into()])
                .unwrap()
                .len(),
            1
        );
        assert!(choose_targets(&snapshot, &["a".into(), "b".into()]).is_err());
        snapshot.targets.get_mut("a").unwrap().item.cleanable = false;
        assert!(choose_targets(&snapshot, &["a".into()]).is_err());
        remove_cleaned_targets(
            &mut snapshot,
            &[ItemOutcome {
                title: "b".into(),
                path: nested.display().to_string(),
                status: "trashed".into(),
                message: String::new(),
            }],
        );
        assert_eq!(snapshot.id, "test");
        assert_eq!(snapshot.targets.len(), 1);
        snapshot.targets.get_mut("a").unwrap().item.cleanable = true;
        assert!(choose_targets(&snapshot, &["a".into()]).is_ok());
        assert!(choose_targets(&snapshot, &["b".into()]).is_err());
    }

    #[test]
    fn streamed_targets_are_available_in_snapshot_before_scan_finishes() {
        let state = Arc::new(State::default());
        let (guard, control) = begin_scan(&state, ScanMode::Projects, "live-projects").unwrap();
        state.data.lock().unwrap().snapshots.insert(
            ScanMode::Projects,
            Snapshot {
                id: "live-projects".into(),
                mode: ScanMode::Projects,
                targets: HashMap::new(),
                children: HashMap::new(),
            },
        );
        let temp = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(temp.path()).unwrap();
        let target = Target {
            item: Candidate {
                id: "item-0".into(),
                title: "cache".into(),
                path: path.display().to_string(),
                category: "项目产物".into(),
                description: String::new(),
                bytes: 1,
                entries: 1,
                modified_at: None,
                is_dir: true,
                cleanable: true,
                recommended: true,
                status: "ready".into(),
                reason: "fixture".into(),
                complete: true,
                protection: None,
                excluded_by: None,
            },
            identity: Some(safety::capture(&path).unwrap()),
            owner_patterns: vec![],
        };
        state
            .data
            .lock()
            .unwrap()
            .snapshots
            .get_mut(&ScanMode::Projects)
            .unwrap()
            .targets
            .insert(target.item.id.clone(), target);
        control.pause();
        assert_eq!(snapshot_items(&state, "live-projects").unwrap().len(), 1);
        assert!(choose_snapshot_targets(&state, "live-projects", &["item-0".into()]).is_err());
        control.resume();
        drop(guard);
        assert_eq!(
            choose_snapshot_targets(&state, "live-projects", &["item-0".into()])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn saving_settings_does_not_discard_existing_snapshots() {
        let state = Arc::new(State::default());
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let cache = home.join("cache");
        std::fs::create_dir(&cache).unwrap();
        let target = Target {
            item: Candidate {
                id: "item-0".into(),
                title: "cache".into(),
                path: cache.display().to_string(),
                category: "开发缓存".into(),
                description: String::new(),
                bytes: 1,
                entries: 1,
                modified_at: None,
                is_dir: true,
                cleanable: true,
                recommended: true,
                status: "ready".into(),
                reason: "fixture".into(),
                complete: true,
                protection: None,
                excluded_by: None,
            },
            identity: Some(safety::capture(&cache).unwrap()),
            owner_patterns: vec![],
        };
        state.data.lock().unwrap().snapshots.insert(
            ScanMode::Quick,
            Snapshot {
                id: "keep".into(),
                mode: ScanMode::Quick,
                children: HashMap::new(),
                targets: HashMap::from([("item-0".into(), target)]),
            },
        );
        let store = Storage(home.join("state"));
        let updated = persist_settings(
            &state,
            &store,
            Settings {
                excluded_paths: vec![cache.join("keep").display().to_string()],
                ..Default::default()
            },
        );
        assert!(updated.is_ok());
        let data = state.data.lock().unwrap();
        let candidate = &data.snapshots[&ScanMode::Quick].targets["item-0"].item;
        assert_eq!(candidate.protection.as_deref(), Some("manual"));
        assert!(!candidate.cleanable);
        drop(data);
        persist_settings(&state, &store, Settings::default()).unwrap();
        let data = state.data.lock().unwrap();
        let candidate = &data.snapshots[&ScanMode::Quick].targets["item-0"].item;
        assert_eq!(candidate.protection.as_deref(), Some("unknown"));
        assert!(!candidate.cleanable);
    }

    #[test]
    fn project_proof_protects_tracked_and_nested_repositories() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        assert!(
            safety::command_output("/usr/bin/git", &["init", "-q"], Some(&root))
                .unwrap()
                .0
        );
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(root.join("target/source"), b"source").unwrap();
        assert!(safety::project_proof(&root.join("target")).is_err());
        safety::command_output("/usr/bin/git", &["add", "target/source"], Some(&root)).unwrap();
        assert!(safety::project_proof(&root.join("target")).is_err());
        std::fs::create_dir(root.join("node_modules")).unwrap();
        std::fs::write(root.join(".gitignore"), b"node_modules/\n").unwrap();
        assert!(safety::project_proof(&root.join("node_modules")).is_ok());
    }
}
