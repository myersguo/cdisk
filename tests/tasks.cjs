// Vite must be running. No native deletion; this exercises the explicit browser demo.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const assert = require("node:assert/strict");

(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1180, height: 780 } });
    await page.addInitScript(() => localStorage.setItem("cdisk.locale", "zh"));
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.locator(".candidate-row").first().waitFor();
    await page.getByRole("button", { name: "暂停扫描", exact: true }).click();
    await page.getByRole("button", { name: "继续扫描", exact: true }).waitFor();
    const pausedCount = await page.locator(".candidate-row").count();
    const pausedProgress = await page.locator(".scan-progress strong").innerText();
    await page.getByLabel("选择 Go 编译缓存", { exact: true }).check();
    assert.ok(await page.getByRole("button", { name: "预览清理", exact: true }).isEnabled());
    await page.getByRole("button", { name: "预览清理", exact: true }).click();
    await page.getByRole("dialog").waitFor();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.ok(await page.getByLabel("选择 Go 编译缓存", { exact: true }).isChecked());
    await page.getByRole("button", { name: "重新扫描", exact: true }).click();
    await page.locator(".candidate-row").first().waitFor();
    await page.getByRole("button", { name: "暂停扫描", exact: true }).click();
    await page.getByRole("button", { name: "继续扫描", exact: true }).waitFor();
    await page.waitForTimeout(400);
    assert.equal(await page.locator(".candidate-row").count(), pausedCount);
    assert.equal(await page.locator(".scan-progress strong").innerText(), pausedProgress);

    await page.getByRole("button", { name: "项目瘦身", exact: true }).click();
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.locator(".candidate-row").first().waitFor();
    assert.match(await page.locator(".candidate-row").first().innerText(), /project/);
    assert.match(await page.locator("nav").innerText(), /暂停/);
    await page.getByRole("button", { name: /日常清理/ }).click();
    assert.equal(await page.locator(".candidate-row").count(), pausedCount);
    await page.getByRole("button", { name: "继续扫描", exact: true }).click();
    await page.getByRole("button", { name: "暂停扫描", exact: true }).waitFor();
    await page.waitForTimeout(210);
    await page.getByRole("button", { name: "暂停扫描", exact: true }).click();
    await page.getByRole("button", { name: "继续扫描", exact: true }).waitFor();
    assert.ok(await page.locator(".candidate-row").count() >= pausedCount);
    await page.getByRole("button", { name: "取消扫描", exact: true }).click();
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    await page.getByText("扫描已取消。已完整扫描且通过安全检查的项目仍可清理；未完成的项目需重新检查。").waitFor();
    assert.ok(await page.locator(".candidate-row").count() > 0);
    await page.getByLabel("选择 Go 编译缓存", { exact: true }).check();
    assert.ok(await page.getByRole("button", { name: "预览清理", exact: true }).isEnabled());
    await page.getByRole("button", { name: "预览清理", exact: true }).click();
    await page.getByRole("dialog").waitFor();
    await page.keyboard.press("Escape");

    await page.getByRole("button", { name: /项目瘦身/ }).click();
    if (await page.getByRole("button", { name: "取消扫描", exact: true }).count()) {
      await page.getByRole("button", { name: "取消扫描", exact: true }).click();
    }
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    await page.getByRole("button", { name: /日常清理/ }).click();
    assert.ok(await page.getByLabel("选择 Go 编译缓存", { exact: true }).isChecked());

    await page.getByRole("button", { name: "安装包", exact: true }).click();
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.match(await page.locator(".detail-panel").innerText(), /镜像已挂载/);
    assert.equal(await page.getByRole("button", { name: "移除手动保护并检查", exact: true }).count(), 0);
    assert.ok(await page.getByLabel("选择 Editor.dmg", { exact: true }).isDisabled());
    await page.getByRole("button", { name: "重新检查此项", exact: true }).click();
    await page.getByText("此项已重新检查。", { exact: true }).waitFor();
    assert.ok(await page.getByLabel("选择 Editor.dmg", { exact: true }).isDisabled());
    await page.screenshot({ path: "/tmp/cdisk-mounted-v04.png" });

    await page.getByRole("button", { name: "扫描与保护", exact: true }).click();
    await page.getByLabel("保护名单", { exact: true }).fill("/Users/demo/.go");
    await page.getByRole("button", { name: "保存设置", exact: true }).click();
    await page.getByText("已保存。已有扫描结果会保留，清理前会按最新保护规则检查。").waitFor();
    await page.getByRole("button", { name: "日常清理", exact: true }).click();
    assert.ok(await page.locator(".candidate-row").count() > 0);
    assert.match(await page.locator(".detail-panel").innerText(), /手动保护/);
    await page.getByRole("button", { name: "移除手动保护并检查", exact: true }).click();
    await page.getByText("手动保护已移除，已重新检查；其他安全限制仍有效。").waitFor();
    assert.ok(await page.getByLabel("选择 Go 编译缓存", { exact: true }).isEnabled());
    assert.equal(await page.getByRole("button", { name: "移除手动保护并检查", exact: true }).count(), 0);
    assert.deepEqual(errors, []);
    console.log("PASS concurrent pages, retained selections, pause/resume, cancel while paused, partial cleanup, mounted gate, manual unprotect");
    await page.close();

    const nativePage = await browser.newPage({ viewport: { width: 1180, height: 780 } });
    await nativePage.addInitScript(() => {
      localStorage.setItem("cdisk.locale", "zh");
      let nextCallback = 1;
      const callbacks = {};
      const eventHandlers = {};
      const jobs = {};
      const snapshots = {};
      const preparations = {};
      let progressHandler;
      window.__taskCalls = [];
      const disk = { totalBytes: 500e9, availableBytes: 100e9 };
      window.__TAURI_INTERNALS__ = {
        transformCallback(fn) { const id = nextCallback++; callbacks[id] = fn; return id; },
        unregisterCallback(id) { delete callbacks[id]; },
        metadata: { currentWindow: { label: "main" } },
        async invoke(command, args = {}) {
          window.__taskCalls.push({ command, args });
          if (command === "bootstrap") return { disk, home: "/fixture", history: [], settings: { projectRoots: [], excludedPaths: [] } };
          if (command === "plugin:event|listen") {
            eventHandlers[args.event] = callbacks[args.handler];
            if (args.event === "scan-progress") progressHandler = callbacks[args.handler];
            return nextCallback++;
          }
          if (command === "plugin:event|unlisten") return;
          if (command === "scan_disk") return new Promise(resolve => {
            const item = {
              id: "item-0", title: `${args.mode} cache`, path: `/fixture/${args.mode}`, category: "开发缓存",
              bytes: 4096, entries: 1, modifiedAt: 1, isDir: true, cleanable: true, recommended: true,
              complete: true, protection: null, excludedBy: null, status: "ready", reason: "fixture", description: "fixture",
            };
            const report = { scanId: args.scanId, mode: args.mode, root: "/fixture", parent: "/", disk,
              scannedEntries: 1, unreadableEntries: 0, skippedEntries: 0, cancelled: false,
              truncated: false, elapsedMs: 1, candidates: [item] };
            jobs[args.scanId] = { resolve, report };
            setTimeout(() => progressHandler({ payload: {
              scanId: args.scanId, mode: args.mode, scannedEntries: 1, unreadableEntries: 0,
              currentPath: item.path, candidate: item,
            } }), 30);
          });
          if (command === "pause_scan" || command === "resume_scan") return !!jobs[args.scanId];
          if (command === "scan_candidates") return jobs[args.scanId]?.report.candidates ?? snapshots[args.scanId]?.candidates ?? [];
          if (command === "save_settings") return { settings: args.settings };
          if (command === "cancel_scan") {
            const job = jobs[args.scanId];
            if (!job) return false;
            snapshots[args.scanId] = job.report;
            job.resolve({ ...job.report, cancelled: true });
            delete jobs[args.scanId];
            return true;
          }
          if (command === "prepare_cleanup") {
            const report = snapshots[args.scanId];
            if (!report) throw new Error("Wrong/overwritten snapshot");
            if (args.candidateIds[0] !== "item-0") throw new Error("Wrong selection");
            return new Promise((resolve, reject) => {
              preparations[args.scanId] = { reject };
              setTimeout(() => eventHandlers["cleanup-validation-progress"]?.({ payload: {
                scanId: args.scanId, completed: 1, total: 231, currentPath: report.candidates[0].path,
              } }), 30);
              setTimeout(() => {
                if (!preparations[args.scanId]) return;
                delete preparations[args.scanId];
                resolve({ token: "fixture", items: report.candidates, estimatedBytes: 4096 });
              }, 500);
            });
          }
          if (command === "cancel_prepare_cleanup") {
            const preparation = preparations[args.scanId];
            if (!preparation) return false;
            delete preparations[args.scanId];
            preparation.reject("预览校验已取消");
            return true;
          }
          throw new Error(`Unexpected IPC: ${command}`);
        },
      };
    });
    await nativePage.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await nativePage.getByRole("button", { name: "开始扫描", exact: true }).click();
    await nativePage.getByText("quick cache", { exact: true }).first().waitFor();
    await nativePage.getByRole("button", { name: "暂停扫描", exact: true }).click();
    await nativePage.getByRole("button", { name: "继续扫描", exact: true }).waitFor();
    await nativePage.getByLabel("选择 quick cache", { exact: true }).check();
    assert.ok(await nativePage.getByRole("button", { name: "预览清理", exact: true }).isEnabled());
    await nativePage.getByRole("button", { name: "预览清理", exact: true }).click();
    await nativePage.getByText("正在重新校验 1 / 231", { exact: true }).waitFor();
    await nativePage.getByRole("button", { name: "取消校验", exact: true }).click();
    await nativePage.getByText("已取消预览校验，没有执行清理。", { exact: true }).waitFor();
    assert.equal(await nativePage.getByRole("dialog").count(), 0);

    await nativePage.getByRole("button", { name: "项目瘦身", exact: true }).click();
    await nativePage.getByRole("button", { name: "开始扫描", exact: true }).click();
    await nativePage.getByText("projects cache", { exact: true }).first().waitFor();
    await nativePage.getByRole("button", { name: "取消扫描", exact: true }).click();
    await nativePage.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    await nativePage.getByRole("button", { name: /日常清理/ }).click();
    await nativePage.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    assert.ok(await nativePage.getByLabel("选择 quick cache", { exact: true }).isChecked());
    await nativePage.getByRole("button", { name: "重新扫描", exact: true }).click();
    await nativePage.getByText("quick cache", { exact: true }).first().waitFor();
    await nativePage.getByRole("button", { name: "取消扫描", exact: true }).click();
    await nativePage.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
    await nativePage.getByLabel("选择 quick cache", { exact: true }).check();
    await nativePage.getByRole("button", { name: "预览清理", exact: true }).click();
    await nativePage.getByText("正在重新校验 1 / 231", { exact: true }).waitFor();
    await nativePage.getByRole("button", { name: "取消校验", exact: true }).click();
    await nativePage.getByText("已取消预览校验，没有执行清理。", { exact: true }).waitFor();
    assert.equal(await nativePage.getByRole("dialog").count(), 0);
    const calls = await nativePage.evaluate(() => window.__taskCalls);
    const quick = calls.find(c => c.command === "scan_disk" && c.args.mode === "quick").args.scanId;
    const project = calls.find(c => c.command === "scan_disk" && c.args.mode === "projects").args.scanId;
    assert.deepEqual(
      calls.find(c => c.command === "scan_disk" && c.args.mode === "quick").args.dailyCategories,
      ["system", "user", "application", "browser", "logs", "temporary", "downloads", "trash"],
    );
    assert.notEqual(quick, project);
    assert.equal(calls.find(c => c.command === "pause_scan").args.scanId, quick);
    assert.equal(calls.filter(c => c.command === "scan_candidates").length, 1);
    assert.equal(calls.find(c => c.command === "prepare_cleanup").args.scanId, quick);
    assert.equal(calls.find(c => c.command === "cancel_prepare_cleanup").args.scanId, quick);
    assert.ok(calls.filter(c => c.command === "cancel_scan").length >= 2);
    console.log("PASS synthetic native IPC: independent scan IDs, streamed results, pause/resume/cancel, preview progress/cancel");
    await nativePage.close();
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
