// Production Web + native runtime, disposable home, synthetic GitHub responses.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
const repo = fileURLToPath(new URL("../", import.meta.url));
const artifacts = path.join(repo, "dist/desktop-download-acceptance");
await mkdir(artifacts, { recursive: true });
const temp = await mkdtemp(path.join(os.tmpdir(), "lorevia-download-qa-"));
const binary = path.resolve(
  process.env.NATIVE_TEST_BINARY || "target/release/deeptutor-server",
);
let backend,
  frontend,
  browser,
  logs = "";
const freePort = async () => {
  const s = createServer().listen(0, "127.0.0.1");
  await once(s, "listening");
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const stop = async (child) => {
  if (child && child.exitCode === null) {
    const done = once(child, "exit");
    child.kill("SIGTERM");
    await done;
  }
};
const ready = async (url) => {
  for (let i = 0; i < 300; i++) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(logs.slice(-5000));
};
try {
  const bp = await freePort(),
    p = await freePort(),
    origin = `http://127.0.0.1:${p}`;
  backend = spawn(
    binary,
    ["--home", path.join(temp, "home"), "serve", "--port", String(bp)],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  for (const s of [backend.stdout, backend.stderr])
    s.on("data", (b) => (logs += b));
  await ready(`http://127.0.0.1:${bp}/health`);
  const web = path.join(
    repo,
    "web",
    process.env.DEEPTUTOR_NEXT_DIST_DIR || ".next-import",
    "standalone",
  );
  frontend = spawn(process.execPath, [path.join(web, "server.js")], {
    cwd: web,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      HOSTNAME: "127.0.0.1",
      PORT: String(p),
      DEEPTUTOR_API_BASE_URL: `http://127.0.0.1:${bp}`,
      BACKEND_PORT: String(bp),
      DEEPTUTOR_HOME: path.join(temp, "home"),
      NEXT_TELEMETRY_DISABLED: "1",
    },
  });
  for (const s of [frontend.stdout, frontend.stderr])
    s.on("data", (b) => (logs += b));
  await ready(origin + "/api/native/status");
  const { chromium } = await import(
    path.join(
      repo,
      "packages/browser-testing/node_modules/playwright/index.mjs",
    )
  );
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({
      viewport: { width: 1440, height: 1000 },
    }),
    errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  let state = [],
    status = 200;
  const repository = "https://github.com/yertYang/Lorevia";
  const asset = (name) => ({
    name,
    size: 524288000,
    state: "uploaded",
    browser_download_url:
      repository + "/releases/download/desktop-preview/" + name,
  });
  const release = (assets, preview = true) => ({
    name: preview ? "Lorevia desktop preview" : "Lorevia stable",
    tag_name: preview ? "desktop-preview" : "v1.7.1",
    draft: false,
    prerelease: preview,
    html_url:
      repository + "/releases/tag/" + (preview ? "desktop-preview" : "v1.7.1"),
    assets,
  });
  await page.route(
    "https://api.github.com/repos/yertYang/Lorevia/releases?*",
    (route) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(state),
      }),
  );
  await page.goto(origin + "/learning");
  await page.locator("button.dt-workspace-trigger").first().click();
  await page
    .getByRole("menuitem", { name: "Download desktop app", exact: true })
    .click();
  await page.waitForURL(origin + "/downloads");
  await page
    .getByText(
      "Desktop installers have not been published yet. The source download below requires setup and is not a desktop installer.",
      { exact: true },
    )
    .waitFor();
  assert.equal(
    await page
      .getByRole("link", {
        name: /Download DMG|Download installer|Download portable ZIP/,
      })
      .count(),
    0,
  );
  assert.equal(
    await page.getByText("Not available yet", { exact: true }).count(),
    3,
  );
  assert.ok(
    (
      await page
        .getByRole("link", {
          name: "Download source code (requires setup)",
          exact: true,
        })
        .getAttribute("href")
    ).endsWith("/archive/refs/heads/master.tar.gz"),
  );
  await page.screenshot({
    path: path.join(artifacts, "empty.png"),
    scale: "css",
  });
  status = 503;
  await page
    .getByRole("button", { name: "Refresh downloads", exact: true })
    .click();
  await page
    .getByRole("alert")
    .filter({ hasText: "Unable to check desktop downloads" })
    .waitFor();
  assert.equal(
    await page
      .getByRole("link", { name: "All releases on GitHub", exact: true })
      .getAttribute("href"),
    repository + "/releases",
  );
  await page.screenshot({
    path: path.join(artifacts, "unavailable.png"),
    scale: "css",
  });
  status = 200;
  const name = "Lorevia-MacOS-Apple-Silicon-v1.7.1.dmg";
  state = [release([asset(name)])];
  await page
    .getByRole("button", { name: "Refresh downloads", exact: true })
    .click();
  await page.getByText("Preview build", { exact: true }).waitFor();
  assert.equal(
    await page.getByText("Not available yet", { exact: true }).count(),
    2,
  );
  const link = page.getByRole("link", { name: "Download DMG", exact: true });
  assert.equal(
    await link.getAttribute("href"),
    asset(name).browser_download_url,
  );
  await page.route(asset(name).browser_download_url, (route) =>
    route.fulfill({
      contentType: "application/octet-stream",
      headers: { "Content-Disposition": `attachment; filename="${name}"` },
      body: "synthetic-download-fixture",
    }),
  );
  const pending = page.waitForEvent("download");
  await link.click();
  assert.equal((await pending).suggestedFilename(), name);
  await page.screenshot({
    path: path.join(artifacts, "preview.png"),
    scale: "css",
  });
  state = [
    release([asset(name)]),
    release(
      [
        asset("Lorevia-MacOS-Intel-v1.7.1.dmg"),
        asset("Lorevia-Windows-v1.7.1-Setup.exe"),
        asset("Lorevia-Windows-v1.7.1-Portable.zip"),
      ],
      false,
    ),
  ];
  await page
    .getByRole("button", { name: "Refresh downloads", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Download installer", exact: true })
    .waitFor();
  assert.equal(
    await page
      .getByRole("checkbox", { name: "Show preview builds", exact: true })
      .isChecked(),
    false,
  );
  assert.equal(
    await page
      .getByRole("link", { name: "Download DMG", exact: true })
      .getAttribute("href"),
    asset("Lorevia-MacOS-Intel-v1.7.1.dmg").browser_download_url,
  );
  await page
    .getByRole("checkbox", { name: "Show preview builds", exact: true })
    .check();
  await page.getByText("Preview build", { exact: true }).waitFor();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(300);
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  );
  await page.screenshot({
    path: path.join(artifacts, "mobile.png"),
    scale: "css",
  });
  await link.scrollIntoViewIfNeeded();
  await page.screenshot({
    path: path.join(artifacts, "mobile-download.png"),
    scale: "css",
  });
  assert.deepEqual(errors, []);
  await writeFile(
    path.join(artifacts, "result.json"),
    JSON.stringify(
      {
        passed: true,
        checks: [
          "sidebar navigation",
          "honest empty state",
          "source/installer distinction",
          "network failure and retry",
          "partial platform availability",
          "preview/stable selection",
          "real download event with synthetic bytes",
          "mobile fit",
        ],
        limits: [
          "synthetic GitHub responses; actual installer verification is separate",
        ],
      },
      null,
      2,
    ),
  );
  console.log("Desktop download UI acceptance passed");
} finally {
  await browser?.close();
  await stop(frontend);
  await stop(backend);
  await rm(temp, { recursive: true, force: true });
}
