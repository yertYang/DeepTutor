// Synthetic, temporary-home acceptance for native reading. Run after a Rust build.
// WEB_QA=1 additionally exercises the production Web build using browser-testing's Playwright.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer } from "node:net";
import { once } from "node:events";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
const repo = fileURLToPath(new URL("../", import.meta.url));
const binary = path.resolve(
  process.env.NATIVE_TEST_BINARY ||
    "target/debug/deeptutor-server" +
      (process.platform === "win32" ? ".exe" : ""),
);
const temp = await mkdtemp(path.join(os.tmpdir(), "native-reading-qa-"));
const home = path.join(temp, "home");
const artifacts = path.join(repo, "dist/native-reading-acceptance");
await mkdir(artifacts, { recursive: true });
const exec = promisify(execFile);
let backend,
  frontend,
  browser,
  origin,
  logs = "",
  activePage;
// Shared functional/visual inventory: library import; source pagination/search/anchors;
// position/finished toggle and resume; annotation selection/range/colors/edit/delete;
// draft reload and stale-write conflict; clipping/download/export; title/delete;
// mobile fit and long source lines; isolation; source deletion; backup/restore.
const checks = [];
const until = async (fn, timeout = 30000) => {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("Timed out. " + logs.slice(-4000));
};
const freePort = async () => {
  const s = createServer().listen(0, "127.0.0.1");
  await once(s, "listening");
  const port = s.address().port;
  await new Promise((r) => s.close(r));
  return port;
};
const stop = async (child) => {
  if (child && child.exitCode === null) {
    const done = once(child, "exit");
    child.kill("SIGTERM");
    await done;
  }
};
const launch = async (root) => {
  const p = await freePort();
  origin = `http://127.0.0.1:${p}`;
  backend = spawn(binary, ["--home", root, "serve", "--port", String(p)], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  for (const s of [backend.stdout, backend.stderr])
    s.on("data", (b) => (logs += b));
  await until(async () => {
    try {
      return (await fetch(origin + "/health")).ok;
    } catch {
      return false;
    }
  });
};
const req = async (url, body, method = "POST", status = 200) => {
  const r = await fetch(
    origin + url,
    body === undefined
      ? {}
      : {
          method,
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
        },
  );
  const v = await r.json();
  assert.equal(r.status, status, JSON.stringify(v));
  return v;
};
const upload = async (scope, content, create = true, name = "阅读资料") => {
  const body = new FormData();
  body.set("name", name);
  body.append("files", new Blob([content]), "物理.md");
  const r = await fetch(
    origin +
      `/api/knowledge-bases${create ? "" : "/" + encodeURIComponent(name) + "/upload"}?workspace_id=${scope}`,
    { method: "POST", body },
  );
  assert.equal(r.status, 200, await r.text());
  return (
    await req(
      `/api/knowledge-bases/${encodeURIComponent(name)}/files?workspace_id=${scope}`,
    )
  ).files[0];
};
try {
  await launch(home);
  const workspace = (
    await req("/api/settings/workspace/registrations", { name: "阅读验收" })
  ).workspace;
  const scope = workspace.workspace_id,
    q = `?workspace_id=${scope}`;
  const source = Array.from({ length: 430 }, (_, i) =>
    i === 1
      ? "能量守恒，第二行的来源。"
      : i === 200
        ? "长行" + "x".repeat(400)
        : i === 202
          ? '<script>throw new Error("untrusted-source")</script>'
          : `合成阅读第 ${i + 1} 行：能量可以转换。`,
  ).join("\n");
  let file = await upload(scope, source);
  const addBody = { kb: "阅读资料", path: file.name, hash: file.sha256 };
  let item = await req("/api/reading/items" + q, addBody);
  const itemUrl = "/api/reading/items/" + item.id,
    route = (suffix = "") => itemUrl + suffix + q;
  assert.deepEqual(await req("/api/reading/items" + q, addBody), item);
  const original = await fetch(origin + route("/original"));
  assert.equal(await original.text(), source);
  assert.ok(
    original.headers.get("content-disposition").includes("filename*=UTF-8''%"),
  );
  let p = await req(
    route("/progress"),
    { line: 203, completed: false, expected_revision: 0 },
    "PUT",
  );
  assert.equal(p.line, 203);
  await req(
    route("/progress"),
    { line: 1, completed: true, expected_revision: 0 },
    "PUT",
    409,
  );
  await req(
    route("/progress"),
    { line: 431, completed: false, expected_revision: 1 },
    "PUT",
    400,
  );
  item = await req(
    route(),
    { title: "物理原文快照", expected_revision: 0 },
    "PATCH",
  );
  await req(route(), { title: "stale", expected_revision: 0 }, "PATCH", 409);
  const input = {
    command_id: "http-annotation",
    source_id: item.source_id,
    start_line: 1,
    end_line: 2,
    note: "合成批注",
    color: "yellow",
  };
  const a = await req(route("/annotations"), input);
  assert.equal(a.quote, source.split("\n").slice(0, 2).join("\n"));
  assert.deepEqual(await req(route("/annotations"), input), a);
  await req(route("/annotations"), { ...input, note: "changed" }, "POST", 409);
  await req(
    route("/annotations"),
    { ...input, command_id: "range", end_line: 431 },
    "POST",
    400,
  );
  await req(
    route("/annotations"),
    { ...input, command_id: "snapshot", source_id: "wrong" },
    "POST",
    409,
  );
  const updated = await req(
    route("/annotations/" + a.id),
    { expected_revision: 0, note: "人工调整", color: "blue" },
    "PUT",
  );
  assert.deepEqual(await req(route("/annotations"), input), updated);
  await req(
    route("/annotations/" + a.id),
    { expected_revision: 0, note: "stale", color: "pink" },
    "PUT",
    409,
  );
  const clipUrl = route("/annotations/" + a.id + "/note");
  const page = await req(clipUrl, { expected_revision: 1 });
  const changed = await req(
    "/api/documents/" + page.id + q,
    { expected_revision: 0, title: "人工笔记标题" },
    "PUT",
  );
  assert.deepEqual(await req(clipUrl, { expected_revision: 1 }), changed);
  await req(route("/annotations/" + a.id), { expected_revision: 1 }, "DELETE");
  await req(route("/annotations"), input, "POST", 409);
  assert.deepEqual((await req(route("/annotations"))).annotations, []);
  for (const suffix of [
    "",
    "/source",
    "/progress",
    "/annotations",
    "/export",
    "/original",
  ]) {
    const r = await fetch(origin + itemUrl + suffix + "?workspace_id=other");
    assert.equal(r.status, 404, suffix);
  }
  await req(
    itemUrl + "?workspace_id=other",
    { expected_revision: item.revision, title: "other" },
    "PATCH",
    404,
  );
  await req(
    itemUrl + "?workspace_id=other",
    { expected_revision: item.revision },
    "DELETE",
    404,
  );
  await req(itemUrl + "?workspace_id=a&dt_workspace=b", undefined, "GET", 400);
  checks.push(
    "HTTP isolation, immutable identity, independent CAS, exact quotes, retry receipts, deleted annotation tombstone, edited note preservation",
  );
  file = await upload(scope, "新版本资料", false);
  const newer = await req("/api/reading/items" + q, {
    ...addBody,
    hash: file.sha256,
  });
  assert.notEqual(newer.id, item.id);
  await req(
    `/api/knowledge-bases/${encodeURIComponent("阅读资料")}${q}`,
    {},
    "DELETE",
  );
  assert.equal((await req(route("/source"))).content, source);
  assert.equal(await (await fetch(origin + route("/original"))).text(), source);
  const kept = await req(route("/annotations"), {
    ...input,
    command_id: "kept",
  });
  const exported = await req(route("/export"));
  assert.equal(exported.annotations[0].id, kept.id);
  const index = await req("/api/dashboard/learning-index" + q);
  assert.equal(
    index.sources.native_reading.find((v) => v.id === item.id).progress.line,
    203,
  );
  checks.push(
    "source replacement and deletion, original download filename, export, dashboard",
  );
  let browserItem;
  if (process.env.WEB_QA === "1") {
    await upload(scope, source, true, "界面资料");
    const port = await freePort(),
      front = `http://127.0.0.1:${port}`;
    const standalone = path.join(
      repo,
      "web",
      process.env.DEEPTUTOR_NEXT_DIST_DIR || ".next-import",
      "standalone",
    );
    frontend = spawn(process.execPath, [path.join(standalone, "server.js")], {
      cwd: standalone,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        HOSTNAME: "127.0.0.1",
        PORT: String(port),
        DEEPTUTOR_HOME: home,
        DEEPTUTOR_API_BASE_URL: origin,
        BACKEND_PORT: new URL(origin).port,
        NEXT_TELEMETRY_DISABLED: "1",
      },
    });
    for (const s of [frontend.stdout, frontend.stderr])
      s.on("data", (b) => (logs += b));
    await until(async () => {
      try {
        return (await fetch(front + "/api/native/status")).ok;
      } catch {
        return false;
      }
    });
    const { chromium } = await import(
      path.join(
        repo,
        "packages/browser-testing/node_modules/playwright/index.mjs",
      )
    );
    browser = await chromium.launch({ headless: true });
    const page = (activePage = await browser.newPage({
      viewport: { width: 1440, height: 1000 },
    }));
    const errors = [],
      external = [];
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("request", (r) => {
      if (
        !r.url().startsWith(front) &&
        !r.url().startsWith("data:") &&
        !r.url().startsWith("blob:")
      )
        external.push(r.url());
    });
    const button = (name) => page.getByRole("button", { name, exact: true });
    const textbox = (name) => page.getByRole("textbox", { name, exact: true });
    const number = (name) =>
      page.getByRole("spinbutton", { name, exact: true });
    const visible = async (text) =>
      page.getByText(text, { exact: true }).waitFor();
    await page.goto(front + "/learning/reading?dt_workspace=" + scope);
    await page
      .getByRole("combobox", { name: "Knowledge base", exact: true })
      .selectOption("界面资料");
    await page
      .getByRole("combobox", { name: "Document", exact: true })
      .selectOption("物理.md");
    await button("Add to reading library").click();
    await page.waitForURL(/\/learning\/reading\/[a-f0-9]+/);
    await page
      .getByRole("region", { name: "Source text", exact: true })
      .waitFor();
    browserItem = await req(
      "/api/reading/items/" +
        new URL(page.url()).pathname.split("/").at(-1) +
        q,
    );
    const browserUrl = "/api/reading/items/" + browserItem.id;
    const br = (suffix) => browserUrl + suffix + q;
    await page.screenshot({
      path: path.join(artifacts, "reader-initial.png"),
      scale: "css",
    });
    await button("Next").click();
    await page.locator("#L201").waitFor();
    assert.equal(await page.locator("[data-reading-line]").count(), 200);
    await button("Previous").click();
    await page.locator("#L1").waitFor();
    await textbox("Search this document").fill("第二行");
    await button("Find next").click();
    await number("Go to line").fill("203");
    await button("Go").click();
    await page.locator("#L203").waitFor();
    assert.equal(await page.locator("#L203 script").count(), 0);
    await button("Set position to line 203").click();
    await button("Save position").click();
    await visible("Reading position saved");
    await page
      .getByRole("checkbox", { name: "Finished reading", exact: true })
      .check();
    await button("Save position").click();
    await visible("Reading position saved");
    await page.reload();
    await page.locator("#L203").waitFor();
    assert.equal(await number("Saved reading line").inputValue(), "203");
    assert.equal(
      await page
        .getByRole("checkbox", { name: "Finished reading", exact: true })
        .isChecked(),
      true,
    );
    await page
      .getByRole("checkbox", { name: "Finished reading", exact: true })
      .uncheck();
    await button("Save position").click();
    await visible("Reading position saved");
    await button("New annotation").click();
    await number("Start line").fill("202");
    await number("End line").fill("203");
    await textbox("Annotation note").fill("界面批注草稿");
    await page
      .getByRole("combobox", { name: "Highlight color", exact: true })
      .selectOption("green");
    page.once("dialog", (d) => d.accept());
    await page.reload();
    await visible("Annotation draft restored");
    assert.equal(await textbox("Annotation note").inputValue(), "界面批注草稿");
    await page.screenshot({
      path: path.join(artifacts, "annotation-draft.png"),
      scale: "css",
    });
    await button("Save annotation").click();
    await visible("Annotation saved");
    await button("Edit annotation").click();
    await textbox("Annotation note").fill("界面批注已修改");
    await button("Save annotation").click();
    await visible("Annotation saved");
    await button("Save passage as note").click();
    await visible("Passage saved as an editable note");
    const noteHref = await page
      .getByRole("link", { name: "Open note", exact: true })
      .getAttribute("href");
    assert.ok(noteHref.includes("/co-writer/"));
    const readerHref = page.url();
    await page.getByRole("link", { name: "Open note", exact: true }).click();
    await textbox("Page title").waitFor();
    await page.getByText("界面批注已修改", { exact: true }).waitFor();
    await page.screenshot({
      path: path.join(artifacts, "clipped-note.png"),
      scale: "css",
    });
    const citation = page
      .locator(`a[href*="/learning/reading/${browserItem.id}"]`)
      .first();
    assert.ok((await citation.getAttribute("href")).endsWith("#L202"));
    const sourcePopup = page.waitForEvent("popup");
    await citation.click();
    const sourcePage = await sourcePopup;
    await sourcePage.waitForURL(readerHref + "#L202");
    await sourcePage.locator("#L202").waitFor();
    await sourcePage.close();
    await page.goto(readerHref);
    await page.locator("#L203").waitFor();
    // Stale tab edit must retain the draft after a conflict and reload.
    await button("Edit annotation").click();
    await textbox("Annotation note").fill("冲突时保留的草稿");
    let saved = (await req(br("/annotations"))).annotations[0];
    await req(
      br("/annotations/" + saved.id),
      {
        expected_revision: saved.revision,
        note: "另一个标签页的批注",
        color: "pink",
      },
      "PUT",
    );
    await button("Save annotation").click();
    await page
      .getByRole("alert")
      .filter({ hasText: "changed elsewhere" })
      .waitFor();
    assert.equal(
      await textbox("Annotation note").inputValue(),
      "冲突时保留的草稿",
    );
    await button("Reload saved data").click();
    await visible("另一个标签页的批注");
    assert.equal(
      await textbox("Annotation note").inputValue(),
      "冲突时保留的草稿",
    );
    await button("Discard draft").click();
    await button("Edit annotation").click();
    await textbox("Annotation note").fill("冲突后重新编辑");
    await button("Save annotation").click();
    await visible("Annotation saved");
    await page.screenshot({
      path: path.join(artifacts, "reader-annotations.png"),
      scale: "css",
    });
    const dl = page.waitForEvent("download");
    await page
      .getByRole("link", { name: "Download original", exact: true })
      .click();
    const download = await dl;
    assert.equal(download.suggestedFilename(), "物理.md");
    const exportDl = page.waitForEvent("download");
    await page
      .getByRole("link", { name: "Export reading record", exact: true })
      .click();
    assert.equal((await exportDl).suggestedFilename(), "reading.json");
    await page.getByText("Reading document details", { exact: true }).click();
    await textbox("Reading title").fill("界面阅读记录");
    await button("Rename").click();
    await visible("Saved");
    await page.goto(front + "/learning/reading?dt_workspace=" + scope);
    await page
      .getByRole("link", { name: "界面阅读记录", exact: true })
      .waitFor();
    await button("Refresh").click();
    await page.screenshot({
      path: path.join(artifacts, "library.png"),
      scale: "css",
    });
    await page
      .getByRole("link", { name: "Import documents", exact: true })
      .click();
    await page.waitForURL((u) => u.pathname === "/knowledge-bases");
    await page
      .getByRole("navigation", { name: "Knowledge bases", exact: true })
      .getByRole("button")
      .filter({ hasText: "界面资料" })
      .click();
    await button("物理.md").click();
    await button("Add to reading library").click();
    await page.waitForURL(
      (u) => u.pathname === "/learning/reading/" + browserItem.id,
    );
    await page.goto(front + "/learning?dt_workspace=" + scope);
    await page
      .locator(`a[href*="/learning/reading/${browserItem.id}"]`)
      .filter({ hasText: "界面阅读记录" })
      .last()
      .click();
    await button("Resume saved position").click();
    await page.locator("#L203").waitFor();
    await button("Delete annotation").click();
    await visible("Annotation deleted");
    assert.equal((await req(br("/annotations"))).annotations.length, 0);
    // Selection uses actual pointer input; numeric range remains the keyboard/touch fallback.
    await number("Go to line").fill("2");
    await button("Go").click();
    const box = await page.locator("#L2 span").boundingBox();
    await page.mouse.move(box.x + 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 140, box.y + box.height / 2, { steps: 10 });
    await page.mouse.up();
    await page
      .getByRole("form", { name: "Annotation draft", exact: true })
      .waitFor();
    await textbox("Annotation note").fill("实际选文批注");
    await button("Save annotation").click();
    await visible("Annotation saved");
    await page.goto(front + "/settings/progress?dt_workspace=" + scope);
    await page
      .getByRole("link", { name: "界面阅读记录", exact: true })
      .waitFor();
    // Mobile: initial library and annotation state must fit after sidebar transitions settle.
    const mobile = await browser.newPage({
      viewport: { width: 390, height: 844 },
      isMobile: true,
      hasTouch: true,
    });
    await mobile.goto(front + "/learning/reading?dt_workspace=" + scope);
    await mobile
      .getByRole("link", { name: "界面阅读记录", exact: true })
      .waitFor();
    await mobile.waitForTimeout(500);
    await mobile.screenshot({
      path: path.join(artifacts, "library-mobile.png"),
      scale: "css",
    });
    await mobile
      .getByRole("link", { name: "界面阅读记录", exact: true })
      .click();
    await mobile
      .getByRole("region", { name: "Source text", exact: true })
      .waitFor();
    await mobile.waitForTimeout(500);
    await mobile.screenshot({
      path: path.join(artifacts, "reader-mobile.png"),
      scale: "css",
    });
    const sourceBox = await mobile
      .locator("[data-reading-line]")
      .first()
      .boundingBox();
    assert.ok(
      sourceBox.y < 844,
      "Source text must start within the mobile initial viewport",
    );
    const positionToggle = mobile.getByRole("button", {
      name: "Reading position: line 203",
      exact: true,
    });
    await positionToggle.click();
    await mobile
      .getByRole("spinbutton", { name: "Saved reading line", exact: true })
      .fill("204");
    await mobile
      .getByRole("button", { name: "Save position", exact: true })
      .click();
    await mobile.getByText("Reading position saved", { exact: true }).waitFor();
    await mobile
      .getByRole("button", { name: "Reading position: line 204", exact: true })
      .click();
    const side = mobile.getByRole("complementary", {
      name: "Annotations",
      exact: true,
    });
    await side.scrollIntoViewIfNeeded();
    await mobile.screenshot({
      path: path.join(artifacts, "annotations-mobile.png"),
      scale: "css",
    });
    const fit = await mobile.evaluate(() => ({
      viewport: innerWidth,
      document: document.documentElement.scrollWidth,
      regions: [
        ...document.querySelectorAll(
          "[data-feature-page] input,[data-feature-page] textarea,[data-feature-page] article",
        ),
      ].map((e) => {
        const r = e.getBoundingClientRect();
        return { x: r.x, right: r.right, width: r.width };
      }),
    }));
    assert.ok(fit.document <= fit.viewport, JSON.stringify(fit));
    assert.ok(
      fit.regions.every((r) => r.x >= 0 && r.right <= fit.viewport),
      JSON.stringify(fit),
    );
    await writeFile(
      path.join(artifacts, "mobile-fit.json"),
      JSON.stringify(fit, null, 2),
    );
    await mobile.close();
    // Delete only a separate disposable record; preserve the annotated browser record for restore.
    await page.goto(
      front + "/learning/reading/" + newer.id + "?dt_workspace=" + scope,
    );
    await page.getByText("Reading document details", { exact: true }).click();
    await button("Delete reading copy").click();
    await button("Confirm delete").click();
    await page.waitForURL((u) => u.pathname === "/learning/reading");
    await req("/api/reading/items/" + newer.id + q, undefined, "GET", 404);
    await page.goto(
      front +
        "/learning/reading/" +
        browserItem.id +
        "/sessions?dt_workspace=" +
        scope,
    );
    await page
      .getByText(
        "Reading conversations are not yet available in the native workspace. Open the library to read and annotate imported documents.",
        { exact: true },
      )
      .waitFor();
    assert.deepEqual(errors, []);
    assert.deepEqual(external, []);
    checks.push(
      "production browser library/import, pagination, search, anchors, saved position, finished toggle, selection and numeric annotations, edits/deletion, restored draft, stale conflict, clipping, downloads, rename, progress page, mobile visual artifacts/fit, explicit deletion and legacy route fallback",
    );
    await browser.close();
    browser = undefined;
    await stop(frontend);
    frontend = undefined;
  }
  const before = await req(route("/export"));
  const beforeBrowser = browserItem
    ? await req("/api/reading/items/" + browserItem.id + "/export" + q)
    : null;
  await stop(backend);
  backend = undefined;
  const backup = path.join(temp, "backup"),
    restored = path.join(temp, "restored");
  await exec(binary, ["--home", home, "backup", "--output", backup]);
  await exec(binary, ["--home", restored, "restore", "--from", backup]);
  await launch(restored);
  const after = await req(route("/export"));
  assert.deepEqual(after.item, before.item);
  assert.deepEqual(after.annotations, before.annotations);
  assert.equal(after.content, before.content);
  if (browserItem) {
    const afterBrowser = await req(
      "/api/reading/items/" + browserItem.id + "/export" + q,
    );
    assert.deepEqual(afterBrowser.item, beforeBrowser.item);
    assert.deepEqual(afterBrowser.annotations, beforeBrowser.annotations);
  }
  checks.push(
    "real binary backup/restore of snapshots, progress and annotations",
  );
  const result = { ok: true, binary, web: process.env.WEB_QA === "1", checks };
  await writeFile(
    path.join(
      artifacts,
      process.env.WEB_QA === "1" ? "result.json" : "result-http.json",
    ),
    JSON.stringify(result, null, 2),
  );
  console.log(JSON.stringify(result, null, 2));
} catch (e) {
  if (activePage && !activePage.isClosed())
    await activePage
      .screenshot({ path: path.join(artifacts, "failure.png"), scale: "css" })
      .catch(() => {});
  await writeFile(path.join(artifacts, "failure.log"), logs);
  throw e;
} finally {
  await browser?.close();
  await stop(frontend);
  await stop(backend);
  await rm(temp, { recursive: true, force: true });
}
