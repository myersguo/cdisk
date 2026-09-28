// Synthetic native IPC: no real filesystem scans or cleanup.
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
      const node = (id, path, isDir) => ({ id, path, title: path.split("/").at(-1), isDir,
        bytes: 4096, entries: 1, modifiedAt: 1, category: isDir ? "目录" : "文件", description: "fixture",
        cleanable: !isDir, recommended: false, status: isDir ? "partial" : "review", reason: "fixture",
        complete: !isDir, protection: null, excludedBy: null });
      const root = node("analysis-root", "/fixture", true);
      const a = node("a", "/fixture/a", true), b = node("b", "/fixture/b", true);
      const deep = node("deep", "/fixture/a/deep", true);
      const file = node("file", "/fixture/b/file.txt", false), leaf = node("leaf", "/fixture/a/deep/leaf", false);
      const tree = { "analysis-root": { directory: root, children: [a, b] },
        a: { directory: a, children: [deep] }, b: { directory: b, children: [file] },
        deep: { directory: deep, children: [leaf] } };
      window.__analysisCalls = [];
      let finish;
      let scanId;
      const callbacks = {}, handlers = {};
      let nextCallback = 1;
      window.__grow = () => { file.bytes = 65536; b.bytes = 65536; root.bytes = 131072; };
      window.__TAURI_INTERNALS__ = {
        transformCallback(fn) { const id = nextCallback++; callbacks[id] = fn; return id; }, unregisterCallback() {}, metadata: { currentWindow: { label: "main" } },
        async invoke(command, args = {}) {
          window.__analysisCalls.push({ command, args });
          if (command === "bootstrap") return { disk, home: "/fixture", history: [], settings: { projectRoots: [], excludedPaths: [] } };
          if (command === "plugin:event|listen") { handlers[args.event] = callbacks[args.handler]; return 1; }
          if (command === "plugin:event|unlisten") return;
          if (command === "scan_disk") {
            scanId = args.scanId;
            setTimeout(() => handlers["scan-progress"]?.({ event: "scan-progress", id: 1, payload: {
              scanId, mode: "full", currentPath: "/fixture/a/deep", scannedEntries: 6, unreadableEntries: 0,
            } }), 20);
            return new Promise(resolve => { finish = () => resolve({ scanId, mode: "full", root: "/fixture", parent: "/", disk,
              candidates: [a, b], scannedEntries: 6, unreadableEntries: 0, skippedEntries: 0, cancelled: true, truncated: false, elapsedMs: 1000 }); });
          }
          if (command === "analysis_directory") {
            if (args.scanId !== scanId) throw new Error("wrong snapshot");
            const result = structuredClone({ ...tree[args.candidateId || "analysis-root"], truncated: false });
            if (args.candidateId === "a" && window.__delayA) {
              window.__delayA = false;
              return new Promise(resolve => { window.__releaseA = () => resolve(result); });
            }
            return result;
          }
          if (command === "pause_scan" || command === "resume_scan") return true;
          if (command === "cancel_scan") { finish(); return true; }
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    });
    await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "磁盘分析", exact: true }).click();
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.locator(".current-column .candidate-row").first().waitFor();
    await page.locator('.current-column .candidate-open[title="/fixture/a"]').click();
    await page.locator(".current-column .candidate-open").filter({ hasText: "deep" }).waitFor();
    await page.locator(".current-column .candidate-open").filter({ hasText: "deep" }).click();
    await page.getByLabel("选择 leaf", { exact: true }).waitFor();
    assert.equal(await page.locator(".analysis-column").count(), 3);
    assert.ok(await page.getByRole("button", { name: "暂停扫描", exact: true }).isVisible());
    assert.ok(await page.getByLabel("选择 leaf", { exact: true }).isDisabled(), "cleanup remains gated while scanning");
    await page.screenshot({ path: "/tmp/cdisk-analysis-live.png" });
    await page.locator('.ancestor-column .candidate-open[title="/fixture/b"]').click();
    await page.getByLabel("选择 file.txt", { exact: true }).waitFor();
    assert.equal(await page.locator(".analysis-column").count(), 2);
    await page.evaluate(() => window.__grow());
    await page.locator(".current-column .analysis-size").filter({ hasText: "64 KiB" }).waitFor();
    await page.getByRole("button", { name: "暂停扫描", exact: true }).click();
    await page.getByRole("button", { name: "继续扫描", exact: true }).waitFor();
    await page.getByLabel("选择 file.txt", { exact: true }).check();
    await page.getByRole("button", { name: "上一级目录", exact: true }).click();
    assert.equal(await page.getByLabel("分析目录").inputValue(), "/fixture");
    assert.ok(await page.getByLabel("选择 a", { exact: true }).isDisabled(), "unfinished directories stay protected while paused");
    await page.getByRole("button", { name: "继续扫描", exact: true }).click();
    await page.evaluate(() => { window.__delayA = true; });
    await page.locator('.current-column .candidate-open[title="/fixture/a"]').click();
    await page.waitForFunction(() => typeof window.__releaseA === "function");
    await page.locator('.ancestor-column .candidate-open[title="/fixture/b"]').click();
    await page.evaluate(() => window.__releaseA());
    await page.getByLabel("选择 file.txt", { exact: true }).waitFor();
    assert.equal(await page.getByLabel("分析目录").inputValue(), "/fixture/b", "late previous-directory responses are ignored");
    await page.getByRole("button", { name: "取消扫描", exact: true }).click();
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.equal(await page.getByLabel("分析目录").inputValue(), "/fixture/b", "completion retains the viewed directory");
    const calls = await page.evaluate(() => window.__analysisCalls);
    assert.equal(calls.filter(call => call.command === "scan_disk").length, 1, "navigation never starts another scan");
    assert.equal(calls.filter(call => call.command === "scan_candidates").length, 0, "pausing retains the viewed subtree");
    assert.ok(calls.some(call => call.command === "analysis_directory" && call.args.candidateId === "deep" && call.args.prioritize));
    assert.ok(calls.filter(call => call.command === "analysis_directory").every(call => !Object.hasOwn(call.args, "root")));
    assert.equal(await page.getByRole("alert").count(), 0);
    console.log("PASS live analysis navigation, growing sizes, paused browsing, stale-response rejection, single scan and ID-only queries");
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
