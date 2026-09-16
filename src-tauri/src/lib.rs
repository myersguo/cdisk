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
}

struct Plan {
    token: String,
    scan_id: String,
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
) -> Result<ScanReport, String> {
    validate_id(&scan_id)?;
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
            },
        );
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let snapshot_state = Arc::clone(&state);
        let snapshot_id = scan_id.clone();
        let (report, targets) = scanner::run(
            &scan_id,
            mode,
            root.as_deref(),
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
                            snapshot.targets.insert(target.item.id.clone(), target);
                            if snapshot.targets.len() > scanner::MAX_RESULTS {
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
        state
            .data
            .lock()
            .map_err(|_| "状态不可用")?
            .snapshots
            .insert(
                mode,
                Snapshot {
                    id: scan_id,
                    mode,
                    targets: targets
                        .into_iter()
                        .map(|t| (t.item.id.clone(), t))
                        .collect(),
                },
            );
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
        if snapshot.mode == ScanMode::Full {
            continue;
        }
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
        .find(|rule| path.starts_with(rule))
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
    let settings = store.settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let home = scanner::home()?;
        let total = targets.len();
        validate_in_parallel(
            &targets,
            &control,
            |target| scanner::cleanup_precheck(target, &home, &settings),
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
        for (index, target) in plan.targets.iter().enumerate() {
            history.items[index].status = "running".into();
            history.items[index].message = "正在执行最终安全检查；尚未删除".into();
            store.record(&history)?;
            // Rebuild runtime/mount evidence for every item immediately before deletion.
            let eligibility = scanner::cleanup_check(target, &home, &settings);
            let outcome = eligibility.and_then(|_| {
                history.items[index].message = "正在处理；若应用中断，请检查路径现状".into();
                store.record(&history)?;
                safety::remove(
                    target.identity.as_ref().ok_or("缺少路径身份")?,
                    &format!("{}-{index}", plan.token),
                )
            });
            match outcome {
                Ok(()) => {
                    history.items[index].status = "removed".into();
                    history.items[index].message = "已永久清理".into();
                    history.estimated_bytes =
                        history.estimated_bytes.saturating_add(target.item.bytes);
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
            if let Err(error) = store.record(&history) {
                history.status = "partial".into();
                history.items[index]
                    .message
                    .push_str(&format!("；记录保存失败，已停止：{error}"));
                break;
            }
        }
        if history.status == "running" {
            history.status = if history.items.iter().all(|i| i.status == "removed") {
                "complete"
            } else {
                "partial"
            }
            .into();
        }
        history.available_after = scanner::disk().ok().map(|d| d.available_bytes);
        if let Err(error) = store.record(&history) {
            history.status = "partial".into();
            if let Some(item) = history.items.last_mut() {
                item.message
                    .push_str(&format!("；最终记录保存失败：{error}"));
            }
        }
        // Other pages keep their snapshots; every future cleanup revalidates current paths.
        state
            .data
            .lock()
            .map_err(|_| "状态不可用")?
            .snapshots
            .retain(|_, snapshot| snapshot.id != plan.scan_id);
        Ok(history)
    })
    .await
    .map_err(|e| e.to_string())?
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
                },
            );
            data.pending = Some(Plan {
                token: "plan".into(),
                scan_id: "quick-result".into(),
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
        let snapshot = Snapshot {
            id: "scan".into(),
            mode: ScanMode::Quick,
            targets: HashMap::new(),
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
                targets: HashMap::from([("item-0".into(), target)]),
            },
        );
        let store = Storage(home.join("state"));
        let updated = persist_settings(
            &state,
            &store,
            Settings {
                excluded_paths: vec![cache.display().to_string()],
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
