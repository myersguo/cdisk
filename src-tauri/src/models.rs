use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    Quick,
    Projects,
    Installers,
    Full,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub path: String,
    pub category: String,
    pub description: String,
    pub bytes: u64,
    pub entries: u64,
    pub modified_at: Option<u64>,
    pub is_dir: bool,
    pub cleanable: bool,
    pub recommended: bool,
    pub status: String,
    pub reason: String,
    pub protection: Option<String>,
    pub excluded_by: Option<String>,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub parent_device: u64,
    pub parent_inode: u64,
}

#[derive(Clone)]
pub struct Target {
    pub item: Candidate,
    pub identity: Option<Identity>,
    pub owner_patterns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStats {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scan_id: String,
    pub mode: ScanMode,
    pub scanned_entries: u64,
    pub unreadable_entries: u64,
    pub current_path: String,
    pub candidate: Option<Candidate>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub scan_id: String,
    pub mode: ScanMode,
    pub root: String,
    pub parent: Option<String>,
    pub disk: DiskStats,
    pub scanned_entries: u64,
    pub unreadable_entries: u64,
    pub skipped_entries: u64,
    pub cancelled: bool,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub project_roots: Vec<String>,
    pub excluded_paths: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPreview {
    pub token: String,
    pub items: Vec<Candidate>,
    pub estimated_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupValidationProgress {
    pub scan_id: String,
    pub completed: usize,
    pub total: usize,
    pub current_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemOutcome {
    pub title: String,
    pub path: String,
    pub status: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub started_at: u64,
    pub status: String,
    pub estimated_bytes: u64,
    pub available_before: u64,
    pub available_after: Option<u64>,
    pub items: Vec<ItemOutcome>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub disk: DiskStats,
    pub home: String,
    pub settings: Settings,
    pub history: Vec<HistoryEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub settings: Settings,
}
