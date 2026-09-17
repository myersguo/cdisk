import { localeTag, t, type MessageKey } from "./locales.ts";

export type ScanMode = "quick" | "projects" | "installers" | "full";
export type DailyCategory =
  "system" | "user" | "application" | "browser" |
  "logs" | "temporary" | "downloads" | "trash";
export type DailyCategoryGroup = "cache" | "hygiene";
export type View = ScanMode | "history" | "settings";
export type Settings = { projectRoots: string[]; excludedPaths: string[] };
export type Disk = { totalBytes: number; availableBytes: number };
export type Candidate = {
  id: string; title: string; path: string; category: string; description: string;
  bytes: number; entries: number; modifiedAt: number | null; isDir: boolean;
  cleanable: boolean; recommended: boolean; status: string; reason: string;
  protection: string | null; excludedBy: string | null; complete: boolean;
};
export type Progress = {
  scanId: string; mode: ScanMode; scannedEntries: number; unreadableEntries: number; currentPath: string;
  candidate?: Candidate | null;
};
export type Report = {
  scanId: string; mode: ScanMode; root: string; parent: string | null; disk: Disk;
  scannedEntries: number; unreadableEntries: number; skippedEntries: number;
  cancelled: boolean; truncated: boolean; elapsedMs: number; candidates: Candidate[];
};
export type Preview = { token: string; items: Candidate[]; estimatedBytes: number };
export type ValidationProgress = {
  scanId: string; completed: number; total: number; currentPath: string;
};
export type Outcome = { title: string; path: string; status: string; message: string };
export type HistoryEntry = {
  id: string; startedAt: number; status: string; estimatedBytes: number;
  availableBefore: number; availableAfter: number | null; items: Outcome[];
};
export type Bootstrap = { disk: Disk; home: string; settings: Settings; history: HistoryEntry[] };
export type SettingsUpdate = { settings: Settings };

export const sections: { id: View; labelKey: MessageKey; detailKey: MessageKey; icon: string }[] = [
  { id: "quick", labelKey: "nav.quick", detailKey: "nav.quickDetail", icon: "clean" },
  { id: "projects", labelKey: "nav.projects", detailKey: "nav.projectsDetail", icon: "code" },
  { id: "installers", labelKey: "nav.installers", detailKey: "nav.installersDetail", icon: "box" },
  { id: "full", labelKey: "nav.full", detailKey: "nav.fullDetail", icon: "disk" },
  { id: "history", labelKey: "nav.history", detailKey: "nav.historyDetail", icon: "history" },
  { id: "settings", labelKey: "nav.settings", detailKey: "nav.settingsDetail", icon: "shield" },
];
export const dailyCategories: { id: DailyCategory; group: DailyCategoryGroup; labelKey: MessageKey; detailKey: MessageKey }[] = [
  { id: "system", group: "cache", labelKey: "daily.system", detailKey: "daily.systemDetail" },
  { id: "user", group: "cache", labelKey: "daily.user", detailKey: "daily.userDetail" },
  { id: "application", group: "cache", labelKey: "daily.application", detailKey: "daily.applicationDetail" },
  { id: "browser", group: "cache", labelKey: "daily.browser", detailKey: "daily.browserDetail" },
  { id: "logs", group: "hygiene", labelKey: "daily.logs", detailKey: "daily.logsDetail" },
  { id: "temporary", group: "hygiene", labelKey: "daily.temporary", detailKey: "daily.temporaryDetail" },
  { id: "downloads", group: "hygiene", labelKey: "daily.downloads", detailKey: "daily.downloadsDetail" },
  { id: "trash", group: "hygiene", labelKey: "daily.trash", detailKey: "daily.trashDetail" },
];
export const isScan = (view: View): view is ScanMode => view !== "history" && view !== "settings";
export const bytes = (value: number): string => {
  if (!Number.isFinite(value)) return "—";
  if (value === 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const index = Math.min(4, Math.floor(Math.log(Math.max(1, Math.abs(value))) / Math.log(1024)));
  return `${(value / 1024 ** index).toLocaleString("en-US", { maximumFractionDigits: index ? 1 : 0 })} ${units[index]}`;
};
export function diskHealth(disk: Disk | null) {
  if (!disk || disk.totalBytes <= 0) return { label: t("health.reading"), tone: "neutral", percent: 0 };
  const free = disk.availableBytes / disk.totalBytes;
  return {
    label: free < 0.05 ? t("health.critical") : free < 0.15 ? t("health.low") : t("health.good"),
    tone: free < 0.05 ? "danger" : free < 0.15 ? "warning" : "good",
    percent: Math.max(0, Math.min(100, (1 - free) * 100)),
  };
}
export function visibleItems(items: Candidate[], query: string, category: string, sort: string) {
  const q = query.trim().toLowerCase();
  return items.filter(item => (!q || `${item.title} ${item.path}`.toLowerCase().includes(q))
    && (category === "" || item.category === category))
    .sort((a, b) => sort === "name" ? a.title.localeCompare(b.title) :
      sort === "recent" ? (b.modifiedAt ?? 0) - (a.modifiedAt ?? 0) : b.bytes - a.bytes);
}
export function selection(items: Candidate[], selected: Set<string>) {
  return items.filter(item => item.cleanable && item.complete && selected.has(item.id));
}
export function dailyCategoryFor(category: string): DailyCategory | null {
  if (category === "系统缓存") return "system";
  if (category === "用户缓存" || category === "开发缓存") return "user";
  if (category === "应用缓存" || category === "日志") return "application";
  if (category === "浏览器数据") return "browser";
  if (category === "日志与诊断") return "logs";
  if (category === "过期临时文件") return "temporary";
  if (category === "下载残留") return "downloads";
  if (category === "废纸篓") return "trash";
  return null;
}
export function dailyScope(items: Candidate[], selected: Set<DailyCategory>) {
  return items.filter(item => {
    const category = dailyCategoryFor(item.category);
    return category === null || selected.has(category);
  });
}
export function applySettings(item: Candidate, settings: Settings): Candidate {
  const excludedBy = settings.excludedPaths.find(rule =>
    item.path === rule || item.path.startsWith(`${rule}/`)) ?? null;
  if (excludedBy) {
    return {
      ...item,
      cleanable: false,
      recommended: false,
      status: "manual",
      reason: "已加入保护名单",
      protection: "manual",
      excludedBy,
    };
  }
  if (item.protection === "manual") {
    return {
      ...item,
      cleanable: false,
      recommended: false,
      status: "unknown",
      reason: "保护规则已更新，请重新检查此项",
      protection: "unknown",
      excludedBy: null,
    };
  }
  return item;
}
export function age(timestamp: number | null) {
  if (!timestamp) return t("age.unknown");
  const days = Math.max(0, Math.floor((Date.now() / 1000 - timestamp) / 86400));
  return days === 0 ? t("age.today") : t("age.days", { count: days });
}
const statusKeys: Record<string, MessageKey> = {
  ready: "status.ready", review: "status.review", protected: "status.protected", manual: "status.manual",
  busy: "status.busy", app_running: "status.app_running", mounted: "status.mounted", system: "status.system",
  unknown: "status.unknown", locate: "status.locate", partial: "status.partial",
};
export const statusLabel = (status: string) => statusKeys[status] ? t(statusKeys[status]) : status;
const categoryKeys: Record<string, MessageKey> = {
  "系统缓存": "category.system", "用户缓存": "category.user", "应用缓存": "category.application",
  "浏览器数据": "category.browser", "开发缓存": "category.user", "日志": "category.application",
  "日志与诊断": "category.logs", "过期临时文件": "category.temporary",
  "下载残留": "category.downloads", "废纸篓": "category.trash",
  "项目产物": "category.project", "安装包": "category.installer", "目录": "category.folder", "文件": "category.file",
};
export const categoryLabel = (category: string) => categoryKeys[category] ? t(categoryKeys[category]) : category;
export const formatCount = (value: number) => value.toLocaleString(localeTag());
export function parsePaths(text: string) { return [...new Set(text.split("\n").map(p => p.trim()).filter(Boolean))]; }

export type ScanTask = {
  id: string; state: "idle" | "running" | "paused" | "cancelling" | "done";
  report: Report | null; progress: Progress | null; candidates: Candidate[];
  selected: Set<string>; activeId: string; query: string; category: string; sort: string;
  error: string; notice: string; root: string; controlPending: boolean;
};
export const emptyTask = (): ScanTask => ({
  id: "", state: "idle", report: null, progress: null, candidates: [], selected: new Set(),
  activeId: "", query: "", category: "", sort: "size", error: "", notice: "", root: "", controlPending: false,
});
export const taskActive = (task: ScanTask) => ["running", "paused", "cancelling"].includes(task.state);
export function mergeProgress(task: ScanTask, progress: Progress): ScanTask {
  if (task.id !== progress.scanId || !taskActive(task)) return task;
  let candidates = task.candidates;
  if (progress.candidate) {
    const item = progress.candidate;
    candidates = [...candidates.filter(i => i.id !== item.id), item]
      .sort((a, b) => b.bytes - a.bytes).slice(0, 500);
  }
  return { ...task, progress, candidates, activeId: task.activeId || candidates[0]?.id || "" };
}
