import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Bootstrap, Candidate, Progress, Report, ScanMode, Settings, SettingsUpdate, ValidationProgress } from "./domain";

export const native = "__TAURI_INTERNALS__" in window;
const home = "/Users/demo";
const disk = { totalBytes: 494384795648, availableBytes: 78383153152 };
let previewSettings: Settings = { projectRoots: [], excludedPaths: [] };
export const call = <T,>(command: string, args?: Record<string, unknown>) => invoke<T>(command, args);
export const onProgress = (callback: (p: Progress) => void) =>
  native ? listen<Progress>("scan-progress", event => callback(event.payload)) : Promise.resolve(() => {});
export const onValidationProgress = (callback: (p: ValidationProgress) => void) =>
  native ? listen<ValidationProgress>("cleanup-validation-progress", event => callback(event.payload)) : Promise.resolve(() => {});
export async function bootstrap(): Promise<Bootstrap> {
  if (native) return call("bootstrap");
  return { disk, home, settings: previewSettings, history: [] };
}
export async function saveSettings(settings: Settings): Promise<SettingsUpdate> {
  if (native) return call("save_settings", { settings });
  previewSettings = settings;
  return { settings };
}
const fixtures: [string, string, number, string, string][] = [
  ["Go 编译缓存", ".go/build-cache", 8.4, "开发缓存", "ready"],
  ["Homebrew 下载缓存", "Library/Caches/Homebrew", 2.2, "开发缓存", "ready"],
  ["Chrome 网页缓存", "Library/Caches/Google/Chrome", 1.3, "应用缓存", "app_running"],
  ["uv Python 缓存", ".cache/uv", 0.8, "开发缓存", "review"],
  ["Codex 日志", "Library/Logs/com.openai.codex", 0.4, "日志", "app_running"],
  ["pip 下载缓存", "Library/Caches/pip", 0.3, "开发缓存", "ready"],
];
function item(title: string, path: string, size: number, category: string, status: string, index: number): Candidate {
  const cleanable = ["ready", "review"].includes(status);
  const excludedBy = previewSettings.excludedPaths.find(p => path === p || path.startsWith(`${p}/`)) ?? null;
  return { id: `item-${index}`, title, path, bytes: Math.round(size * 1024 ** 3), category, status,
    entries: 1248 + index * 432, modifiedAt: Math.floor(Date.now() / 1000) - (status === "review" ? 86400 : 31 * 86400),
    cleanable, recommended: status === "ready", isDir: !title.endsWith(".dmg"),
    description: "演示数据。原生应用会读取当前磁盘，并重新校验清理条件。",
    complete: true, protection: ["busy", "app_running", "mounted", "unknown"].includes(status) ? status : null, excludedBy,
    reason: status === "mounted" ? "安装镜像仍已挂载；请在 Finder 推出镜像，再点击“重新检查此项”" :
      status === "busy" ? "该路径被 node（PID 123）占用；关闭对应任务后重新检查" :
      status === "app_running" ? "所属应用仍在运行（Chrome）；为保护应用缓存/日志，请退出该应用后重新检查" : status === "locate" ?
      "只读分析，可下钻目录或在 Finder 中查看" : "清理后首次使用需要重建或下载。",
    ...(excludedBy ? { status: "manual", protection: "manual", cleanable: false, recommended: false, reason: "已加入保护名单" } : {}),
  };
}
export type DemoControl = { paused: boolean; cancelled: boolean };
export async function scan(mode: ScanMode, scanId: string, root: string | null,
  control: DemoControl, progress: (p: Progress) => void): Promise<Report> {
  if (native) return call("scan_disk", { mode, scanId, root });
  const base = root || home;
  const candidates = mode === "quick" ? fixtures.map((f, i) => item(f[0], `${home}/${f[1]}`, f[2], f[3], f[4], i)) :
    mode === "projects" ? Array.from({ length: 24 }, (_, i) => item(`project-${i + 1} / target`,
      `${home}/repos/project-${i + 1}/target`, 4.8 - i * 0.17, "项目产物", i % 3 === 0 ? "review" : "ready", i)) :
    mode === "installers" ? ["Editor.dmg", "Design.dmg", "Tools.dmg"].map((name, i) =>
      item(name, `${home}/Downloads/${name}`, 1.8 - i * 0.6, "安装包", i === 0 ? "mounted" : "review", i)) :
    ["Library", "repos", "Documents", "Downloads", "Movies", "Pictures", ".cache"].map((name, i) =>
      item(name, `${base}/${name}`, [48, 31, 18, 5, 2, 1, .3][i], "目录", "locate", i));
  const completed: Candidate[] = [];
  for (const candidate of candidates) {
    await new Promise(resolve => setTimeout(resolve, 180));
    while (control.paused && !control.cancelled) await new Promise(resolve => setTimeout(resolve, 30));
    if (control.cancelled) break;
    completed.push(candidate);
    progress({ scanId, mode, currentPath: candidate.path, scannedEntries: completed.length * 3231, unreadableEntries: 0, candidate });
  }
  return { scanId, mode, root: base, parent: base.slice(0, base.lastIndexOf("/")) || null, disk,
    scannedEntries: completed.length * 3231, unreadableEntries: 0, skippedEntries: 3, elapsedMs: completed.length * 180,
    cancelled: control.cancelled, truncated: false, candidates: completed };
}

export async function recheck(scanId: string, candidate: Candidate, removeManualProtection: boolean): Promise<Candidate> {
  if (native) return call("recheck_item", { scanId, candidateId: candidate.id, removeManualProtection });
  if (removeManualProtection && candidate.excludedBy) {
    previewSettings.excludedPaths = previewSettings.excludedPaths.filter(p => p !== candidate.excludedBy);
    return { ...candidate, cleanable: true, recommended: false, protection: null, excludedBy: null, status: "review", reason: "已移除手动保护，重新检查通过" };
  }
  return candidate; // A mounted/busy demo item is not unlocked by clicking recheck.
}
