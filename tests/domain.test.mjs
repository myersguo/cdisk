import { test } from "node:test";
import assert from "node:assert/strict";
import { applySettings, bytes, categoryLabel, diskHealth, visibleItems, selection, dailyScope, parsePaths, emptyTask, mergeProgress, statusLabel } from "../src/domain.ts";
import { localizeBackendText, resolveLocale, setCurrentLocale, t } from "../src/locales.ts";

const items = [
  { id: "cache", title: "Go cache", path: "/Users/demo/cache", category: "开发缓存", bytes: 100, modifiedAt: 4, cleanable: true, complete: true },
  { id: "private", title: "Secrets", path: "/Users/demo/keep", category: "应用数据", bytes: 200, modifiedAt: 9, cleanable: false, complete: true },
];

test("unmeasured disk never displays a false critical state", () => {
  assert.equal(diskHealth(null).tone, "neutral");
  assert.equal(diskHealth({ totalBytes: 100, availableBytes: 50 }).tone, "good");
  assert.equal(diskHealth({ totalBytes: 100, availableBytes: 3 }).tone, "danger");
});

test("only eligible items contribute to selection", () => {
  assert.deepEqual(selection(items, new Set(["cache", "private"])).map(i => i.id), ["cache"]);
});

test("searches paths, filters category, sorts without mutating source", () => {
  assert.equal(visibleItems(items, "KEEP", "", "size")[0].id, "private");
  assert.equal(visibleItems(items, "", "开发缓存", "size").length, 1);
  assert.equal(visibleItems(items, "", "", "size")[0].id, "private");
  assert.equal(items[0].id, "cache");
});

test("daily cleanup exposes cache and disk hygiene categories", () => {
  setCurrentLocale("en");
  assert.equal(categoryLabel("系统缓存"), "System cache");
  assert.equal(categoryLabel("用户缓存"), "User cache");
  assert.equal(categoryLabel("应用缓存"), "App cache");
  assert.equal(categoryLabel("浏览器数据"), "Browser data");
  assert.equal(categoryLabel("日志与诊断"), "Logs & diagnostics");
  assert.equal(categoryLabel("过期临时文件"), "Expired temporary files");
  assert.equal(categoryLabel("下载残留"), "Download remnants");
  assert.equal(categoryLabel("废纸篓"), "Trash");
});

test("daily cleanup scope hides unselected categories without mutating results", () => {
  const categorized = [
    { ...items[0], category: "系统缓存" },
    { ...items[1], category: "浏览器数据" },
    { ...items[0], id: "trash", category: "废纸篓" },
  ];
  assert.deepEqual(dailyScope(categorized, new Set(["browser", "trash"])).map(item => item.id), ["private", "trash"]);
  assert.equal(categorized.length, 3);
});

test("units and path list are explicit", () => {
  assert.equal(bytes(1024 ** 3), "1 GiB");
  assert.equal(bytes(NaN), "—");
  assert.deepEqual(parsePaths("\n ~/repos \n~/repos\n~/keep"), ["~/repos", "~/keep"]);
});

test("late progress never leaks into another task or replaces a finished snapshot", () => {
  const task = { ...emptyTask(), id: "scan-a", state: "running" };
  const progress = { scanId: "scan-b", mode: "quick", scannedEntries: 99, unreadableEntries: 0, currentPath: "/example", candidate: items[0] };
  assert.equal(mergeProgress(task, progress), task);
  const next = mergeProgress(task, { ...progress, scanId: "scan-a" });
  assert.equal(next.candidates.length, 1);
  assert.equal(task.candidates.length, 0);
  const done = { ...next, state: "done" };
  assert.equal(mergeProgress(done, { ...progress, scanId: "scan-a" }), done);
});

test("cancelled report does not invalidate completed candidates, incomplete items still cannot clean", () => {
  const incomplete = { ...items[0], id: "partial", complete: false, cleanable: true };
  const chosen = selection([...items, incomplete], new Set(["cache", "private", "partial"]));
  assert.deepEqual(chosen.map(i => i.id), ["cache"]);
});

test("settings update existing candidates without discarding the result list", () => {
  const protectedItem = applySettings(items[0], { projectRoots: [], excludedPaths: ["/Users/demo/cache"] });
  assert.equal(protectedItem.protection, "manual");
  assert.equal(protectedItem.cleanable, false);
  const retained = applySettings(items[1], { projectRoots: [], excludedPaths: ["/Users/demo/cache"] });
  assert.equal(retained.id, "private");
  const needsRecheck = applySettings(protectedItem, { projectRoots: [], excludedPaths: [] });
  assert.equal(needsRecheck.protection, "unknown");
  assert.equal(needsRecheck.cleanable, false);
});

test("protection labels distinguish user rules from mount and runtime safety", () => {
  setCurrentLocale("zh");
  assert.equal(statusLabel("manual"), "手动保护");
  assert.equal(statusLabel("mounted"), "镜像已挂载");
  assert.equal(statusLabel("busy"), "路径被占用");
  assert.equal(statusLabel("app_running"), "应用运行中");
  assert.equal(statusLabel("unknown"), "状态未知");
  assert.equal(statusLabel("partial"), "未完整扫描");
});

test("locale follows supported system languages and persists explicit choices", () => {
  assert.equal(resolveLocale(null, "zh-TW"), "zh-Hant");
  assert.equal(resolveLocale(null, "ja-JP"), "ja");
  assert.equal(resolveLocale(null, "de-DE"), "en");
  assert.equal(resolveLocale("fr", "zh-CN"), "fr");

  setCurrentLocale("en");
  assert.equal(t("nav.quick"), "Daily cleanup");
  assert.equal(categoryLabel("开发缓存"), "User cache");
  assert.equal(localizeBackendText("Go 编译缓存"), "Go build cache");
  assert.equal(localizeBackendText("可重新生成；仍需确认后才会永久删除"), "Rebuildable. Permanent removal still requires confirmation.");
  assert.equal(localizeBackendText("只清理指定缓存或日志目录，不清理账号、会话和系统数据。"), "Cleans only the named cache or log folder, never accounts, sessions, or system data.");
  assert.equal(localizeBackendText("该路径被 node（PID 123） 占用；关闭对应任务后重新检查"), "This path is used by node（PID 123）. Close the related task and recheck.");
  assert.equal(localizeBackendText("系统目录、其他用户数据或用户主目录受保护"), "System folders, other users’ data, and your home folder are protected.");
  assert.equal(localizeBackendText("扫描已失效"), "This scan result is no longer valid. Scan again.");
  assert.equal(localizeBackendText("unrecognized os error 99"), "unrecognized os error 99");
  setCurrentLocale("zh");
  assert.equal(t("nav.quick"), "日常清理");
  for (const locale of ["zh-Hant", "ja", "ko", "es", "fr"]) {
    setCurrentLocale(locale);
    assert.notEqual(t("settings.limitsText"), "No sudo, app uninstall, system-service changes, or Trash emptying. The latest 100 cleanup records stay on this Mac.");
    assert.notEqual(t("reason.incomplete"), "This item was not fully scanned or contains unreadable, mounted, or protected content. Recheck it.");
  }
  setCurrentLocale("zh");
});
