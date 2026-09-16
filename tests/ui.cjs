// Run with the Vite server on 1420; PLAYWRIGHT_MODULE may point to a bundled installation.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const assert = require("node:assert/strict");

(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  });
  try {
    for (const [width, height] of [[1180, 780], [940, 640]]) {
      const page = await browser.newPage({ viewport: { width, height } });
      await page.addInitScript(() => localStorage.setItem("cdisk.locale", "zh"));
      const errors = [];
      page.on("pageerror", e => errors.push(e.message));
      await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
      assert.equal(await page.locator(".candidate-row").count(), 0);
      await page.getByRole("button", { name: "开始扫描", exact: true }).click();
      await page.locator(".candidate-row").first().waitFor();
      await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
      assert.equal(await page.locator('input[type=checkbox]:checked').count(), 0);
      assert.equal(await page.locator('input[type=checkbox]:disabled').count(), 2);
      await page.getByRole("button", { name: "选择建议项", exact: true }).click();
      assert.equal(await page.locator('input[type=checkbox]:checked').count(), 3);
      await page.getByRole("button", { name: "预览清理", exact: true }).click();
      await page.getByRole("dialog").waitFor();
      assert.equal(await page.getByRole("button", { name: "演示模式不执行清理" }).isDisabled(), true);
      await page.keyboard.press("Escape");
      assert.equal(await page.getByRole("dialog").count(), 0);
      await page.getByLabel("搜索名称或路径").fill("Chrome");
      assert.equal(await page.locator(".candidate-row").count(), 1);
      await page.getByLabel("搜索名称或路径").fill("");
      await page.screenshot({ path: `/tmp/cdisk-quick-${width}.png` });

      await page.getByRole("button", { name: "项目瘦身", exact: true }).click();
      await page.getByRole("button", { name: "开始扫描", exact: true }).click();
      await page.locator(".candidate-row").last().waitFor();
      await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
      const list = page.locator(".candidate-list");
      await list.hover();
      await page.mouse.wheel(0, 2000);
      await page.waitForTimeout(180);
      assert.ok(await list.evaluate(el => el.scrollTop) > 0, "candidate list scrolls");
      await page.locator(".candidate-open").last().click();
      const detail = page.locator(".detail-panel");
      await detail.hover();
      await page.mouse.wheel(0, 900);
      await page.waitForTimeout(180);
      assert.ok(await detail.evaluate(el => el.scrollTop) > 0, "details scroll independently");

      await page.getByRole("button", { name: "磁盘分析", exact: true }).click();
      await page.getByRole("button", { name: "开始扫描", exact: true }).click();
      await page.locator(".candidate-row").first().waitFor();
      await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
      assert.equal(await page.locator('input[type=checkbox]').count(), 0);
      const before = await page.getByLabel("分析目录").inputValue();
      await page.getByRole("button", { name: "进入目录", exact: true }).click();
      await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
      const after = await page.getByLabel("分析目录").inputValue();
      assert.notEqual(after, before);
      await page.getByRole("button", { name: "上一级目录" }).click();
      await page.getByRole("button", { name: "重新扫描", exact: true }).waitFor();
      assert.equal(await page.getByLabel("分析目录").inputValue(), before);
      await page.getByRole("button", { name: "重新扫描", exact: true }).click();
      await page.getByRole("button", { name: "取消扫描", exact: true }).click();
      await page.getByText("扫描已取消。已完整扫描且通过安全检查的项目仍可清理；未完成的项目需重新检查。").waitFor();
      await page.getByRole("button", { name: "扫描与保护", exact: true }).click();
      await page.getByLabel("开发目录", { exact: true }).fill("~/repos\n~/Projects");
      await page.getByLabel("保护名单", { exact: true }).fill("~/keep");
      await page.getByRole("button", { name: "保存设置", exact: true }).click();
      await page.getByText("已保存。已有扫描结果会保留，清理前会按最新保护规则检查。").waitFor();
      await page.getByRole("button", { name: "操作记录", exact: true }).click();
      await page.getByText("还没有清理记录", { exact: true }).waitFor();
      assert.deepEqual(errors, []);
      console.log(`PASS ${width}x${height}: navigation, search, selection, dialog, independent scroll, drilldown, cancel, settings, history`);
      await page.close();
    }

    // IPC contract test with synthetic data, not a native filesystem operation.
    const page = await browser.newPage({ viewport: { width: 1180, height: 780 } });
    await page.addInitScript(() => {
      localStorage.setItem("cdisk.locale", "zh");
      const callbacks = {};
      const eventHandlers = {};
      let nextId = 1;
      window.__calls = [];
      window.__TAURI_INTERNALS__ = {
        transformCallback(fn) { const id = nextId++; callbacks[id] = fn; return id; },
        unregisterCallback(id) { delete callbacks[id]; },
        metadata: { currentWindow: { label: "main" } },
        async invoke(command, args = {}) {
          window.__calls.push({ command, args });
          const disk = { totalBytes: 500e9, availableBytes: 100e9 };
          const item = {
            id: "safe", title: "Synthetic cache", path: "/fixture/cache", category: "开发缓存",
            description: "IPC fixture", bytes: 4096, entries: 1, modifiedAt: 1, isDir: true,
            cleanable: true, recommended: true, status: "ready", reason: "fixture only",
            complete: true, protection: null, excludedBy: null,
          };
          if (command === "plugin:event|listen") {
            eventHandlers[args.event] = callbacks[args.handler];
            return nextId++;
          }
          if (command === "plugin:event|unlisten") return;
          if (command === "bootstrap") return { disk, home: "/fixture", history: [], settings: { projectRoots: [], excludedPaths: [] } };
          if (command === "scan_disk") {
            if (!args.scanId || !args.mode) throw new Error("Missing scanId/mode");
            return { scanId: args.scanId, mode: args.mode, root: "/fixture", parent: "/", disk,
              scannedEntries: 1, unreadableEntries: 0, skippedEntries: 0, cancelled: false,
              truncated: false, elapsedMs: 1, candidates: [item] };
          }
          if (command === "prepare_cleanup") {
            if (args.candidateIds[0] !== "safe") throw new Error("Wrong candidate");
            return new Promise(resolve => {
              setTimeout(() => eventHandlers["cleanup-validation-progress"]?.({ payload: {
                scanId: args.scanId, completed: 1, total: 1, currentPath: item.path,
              } }), 30);
              setTimeout(() => resolve({ token: "fixture-token", items: [item], estimatedBytes: 4096 }), 90);
            });
          }
          if (command === "execute_cleanup") {
            if (args.token !== "fixture-token") throw new Error("Wrong token");
            return { id: "fixture", startedAt: 1, status: "partial", estimatedBytes: 0,
              availableBefore: 100e9, availableAfter: 100e9,
              items: [{ title: item.title, path: item.path, status: "skipped", message: "fixture: active application" }] };
          }
          throw new Error(`Unexpected IPC ${command}`);
        },
      };
    });
    await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await page.getByRole("button", { name: "开始扫描", exact: true }).click();
    await page.getByLabel("选择 Synthetic cache", { exact: true }).check();
    await page.getByRole("button", { name: "预览清理", exact: true }).click();
    await page.getByText("正在重新校验 1 / 1", { exact: true }).waitFor();
    const permanent = page.getByRole("button", { name: "永久清理所选项", exact: true });
    assert.ok(await permanent.isDisabled());
    await page.getByLabel("我已检查路径，并确认不需要这些内容").check();
    await permanent.click();
    await page.getByRole("heading", { name: "部分完成", exact: true }).waitFor();
    await page.getByText("Synthetic cache", { exact: false }).last().click();
    await page.getByText("fixture: active application", { exact: true }).waitFor();
    assert.equal((await page.evaluate(() => window.__calls)).filter(c => c.command === "execute_cleanup").length, 1);
    console.log("PASS synthetic native IPC: preview, explicit acknowledgement, one execution, skip reason/history");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
