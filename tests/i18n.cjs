const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");
const assert = require("node:assert/strict");

(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
  });
  try {
    const context = await browser.newContext({ locale: "ja-JP", viewport: { width: 1180, height: 780 } });
    const page = await context.newPage();
    await page.goto("http://127.0.0.1:1420", { waitUntil: "networkidle" });
    await page.getByRole("heading", { name: "日常クリーンアップ", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.lang), "ja");

    await page.getByText("言語", { exact: true }).click();
    await page.getByRole("button", { name: /English English/ }).click();
    await page.getByRole("heading", { name: "Daily cleanup", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => localStorage.getItem("cdisk.locale")), "en");

    await page.getByRole("button", { name: "Start scan", exact: true }).click();
    await page.getByText("Go build cache", { exact: true }).first().waitFor();
    await page.getByText("Language", { exact: true }).click();
    await page.getByRole("button", { name: /日本語 Japanese/ }).click();
    await page.getByText("Go ビルドキャッシュ", { exact: true }).first().waitFor();

    await page.getByText("言語", { exact: true }).click();
    await page.getByRole("button", { name: /English English/ }).click();
    await page.reload({ waitUntil: "networkidle" });
    await page.getByRole("heading", { name: "Daily cleanup", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.lang), "en");
    console.log("PASS system locale default, language switch, persistence, document language");
    await context.close();
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
