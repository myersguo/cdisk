// Native IPC is mocked; the browser test never moves real files.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const assert = require("node:assert/strict");

(async () => {
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1180, height: 780 } });
    await page.addInitScript(() => {
      localStorage.setItem("cdisk.locale", "zh");
      const disk = { totalBytes: 500e9, availableBytes: 100e9 };
      const candidates = ["video.mp4", "occupied.txt", "keep.txt", "Library"].map((title, index) => ({
        id: `item-${index}`, title, path: `/fixture/${title}`, category: "文件", description: "fixture",
        bytes: index === 3 ? 16384 : 4096, entries: 1, modifiedAt: 1, isDir: index === 3, complete: true,
        cleanable: index !== 3, recommended: false, status: index === 3 ? "protected" : "review",
        reason: "fixture", protection: null, excludedBy: null,
      }));
      let currentCandidates = candidates;
      let pendingCandidates = [];
      const root = { ...candidates[3], id: "analysis-root", path: "/fixture", title: "fixture" };
      const tree = { "analysis-root": { directory: root, children: candidates, truncated: false },
        "item-3": { directory: candidates[3], children: candidates.map((item, index) => ({ ...item, id: `child-${index}`, path: `/fixture/Library/${item.title}` })), truncated: false } };
      window.__trashCalls = [];
      window.__TAURI_INTERNALS__ = {
        transformCallback() { return 1; }, unregisterCallback() {},
        metadata: { currentWindow: { label: "main" } },
        async invoke(command, args = {}) {
          window.__trashCalls.push({ command, args });
          if (command === "bootstrap") return { disk, home: "/fixture", history: [], settings: { projectRoots: [], excludedPaths: [] } };
          if (command === "plugin:event|listen") return 1;
          if (command === "plugin:event|unlisten") return;
          if (command === "scan_disk") {
            return { scanId: args.scanId, mode: args.mode,
              root: "/fixture", parent: "/", disk, candidates: currentCandidates, scannedEntries: 4, unreadableEntries: 0,
              skippedEntries: 0, cancelled: false, truncated: false, elapsedMs: 1 };
          }
          if (command === "analysis_directory") return structuredClone(tree[args.candidateId || "analysis-root"]);
          if (command === "prepare_cleanup") {
            pendingCandidates = Object.values(tree).flatMap(column => column.children).filter(item => args.candidateIds.includes(item.id));
            return { token: "trash-token", trash: true, items: pendingCandidates, estimatedBytes: 8192 };
          }
          if (command === "execute_cleanup") return new Promise(resolve => {
            window.__finishTrash = () => {
              const removed = pendingCandidates[0];
              for (const column of Object.values(tree)) {
                column.children = column.children.filter(item => item.id !== removed.id);
              }
              for (const item of new Set(Object.values(tree).flatMap(column => [column.directory, ...column.children]))) {
                if (removed.path.startsWith(`${item.path}/`)) item.bytes -= removed.bytes;
              }
              resolve({ id: "trash-token", startedAt: 1, status: "partial",
              estimatedBytes: 4096, availableBefore: disk.availableBytes, availableAfter: disk.availableBytes,
              items: pendingCandidates.map((item, index) => ({ title: item.title, path: item.path,
                status: index ? "skipped" : "trashed", message: index ? "busy" : "已移到废纸篓，可在 Finder 中恢复；空间尚未释放" })) });
            };
          });
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    });
    await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "磁盘分析", exact: true }).click();
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.equal(await page.locator(".candidate-row input:checked").count(), 0);
    assert.ok(await page.getByLabel("选择 Library", { exact: true }).isDisabled());
    await page.locator(".current-column .candidate-open").filter({ hasText: "video.mp4" }).click();
    assert.ok(await page.locator(".detail-panel").isVisible(), "file click opens details without scanning a child");
    assert.equal(await page.locator(".analysis-column").count(), 1);
    await page.getByLabel("选择 video.mp4", { exact: true }).check();
    await page.getByLabel("选择 occupied.txt", { exact: true }).check();
    await page.getByRole("button", { name: "移到废纸篓", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.waitFor();
    assert.match(await dialog.innerText(), /清空废纸篓后才会释放空间/);
    assert.ok(await dialog.getByText("/fixture/video.mp4", { exact: true }).isVisible());
    await dialog.getByRole("checkbox").check();
    await dialog.getByRole("button", { name: "移到废纸篓", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    await page.getByRole("status").filter({ hasText: "正在移到废纸篓" }).waitFor();
    await page.evaluate(() => window.__finishTrash());
    await page.getByText("已移到废纸篓", { exact: true }).waitFor();
    assert.match(await page.locator(".history-page").innerText(), /移到废纸篓不会释放磁盘空间/);
    await page.getByRole("button", { name: "磁盘分析", exact: true }).click();
    assert.equal(await page.locator(".candidate-row").count(), 3);
    assert.equal(await page.getByLabel("选择 video.mp4", { exact: true }).count(), 0);
    assert.ok(await page.getByLabel("选择 occupied.txt", { exact: true }).isChecked());
    assert.ok(await page.getByLabel("选择 keep.txt", { exact: true }).isVisible());
    const calls = await page.evaluate(() => window.__trashCalls.filter(call => ["prepare_cleanup", "execute_cleanup"].includes(call.command)));
    assert.deepEqual(calls[0].args.candidateIds, ["item-0", "item-1"]);
    assert.deepEqual(calls[1].args, { token: "trash-token" });
    await page.locator(".current-column .candidate-open").filter({ hasText: "Library" }).click();
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.equal(await page.locator(".analysis-column").count(), 2);
    await page.getByLabel("选择 video.mp4", { exact: true }).check();
    await page.getByRole("button", { name: "移到废纸篓", exact: true }).click();
    await dialog.waitFor();
    await dialog.getByRole("checkbox").check();
    await dialog.getByRole("button", { name: "移到废纸篓", exact: true }).click();
    await page.getByRole("status").filter({ hasText: "正在移到废纸篓" }).waitFor();
    await page.evaluate(() => window.__finishTrash());
    await page.getByText("已移到废纸篓", { exact: true }).waitFor();
    await page.getByRole("button", { name: "磁盘分析", exact: true }).click();
    assert.equal(await page.locator(".current-column .candidate-row").count(), 3);
    assert.equal(await page.locator(".ancestor-column .branch-active .analysis-size").innerText(), "12 KiB");
    assert.equal(await page.locator(".ancestor-column input").count(), 0, "cached ancestors cannot authorize cleanup");
    console.log("PASS Trash confirmation, background status, retained results, and ID-only IPC");
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
