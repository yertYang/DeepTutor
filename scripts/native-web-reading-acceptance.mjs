// Frozen reading context + synthetic model. LIVE_WEB_QA=1 fetches example.com;
// WEB_QA=1 additionally exercises the production browser interface.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer as httpServer } from "node:http";
import { createServer } from "node:net";
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
const temp = await mkdtemp(path.join(os.tmpdir(), "lorevia-web-reading-"));
const home = path.join(temp, "home");
const artifacts = path.join(repo, "dist/native-web-reading-acceptance");
await mkdir(artifacts, { recursive: true });
let backend,
  frontend,
  browser,
  activePage,
  origin,
  logs = "";
const requests = [],
  checks = [];
const freePort = async () => {
  const s = createServer().listen(0, "127.0.0.1");
  await once(s, "listening");
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const stop = async (c) => {
  if (c && c.exitCode === null) {
    const done = once(c, "exit");
    c.kill("SIGTERM");
    const timer = setTimeout(() => {
      if (c.exitCode === null) c.kill("SIGKILL");
    }, 15000);
    try {
      await done;
    } finally {
      clearTimeout(timer);
    }
  }
};
const until = async (fn, timeout = 30000) => {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error("Timed out. " + logs.slice(-4000));
};
const req = async (url, body, expected = 200) => {
  const r = await fetch(
    origin + url,
    body === undefined
      ? {}
      : {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
        },
  );
  const v = await r.json();
  assert.equal(r.status, expected, JSON.stringify(v));
  return v;
};
const model = httpServer(async (request, response) => {
  let raw = "";
  for await (const chunk of request) raw += chunk;
  const body = JSON.parse(raw);
  requests.push(body);
  const prompt =
    body.messages.filter((m) => m.role === "user").at(-1)?.content || "";
  response.writeHead(200, { "content-type": "text/event-stream" });
  const content = "The selected source says energy is conserved. [L1](#L1)";
  if (prompt.includes("wait for cancellation")) {
    response.write(
      "data: " +
        JSON.stringify({
          choices: [{ delta: { content: "Starting the explanation…" } }],
        }) +
        "\n\n",
    );
    return;
  }
  response.end(
    "data: " +
      JSON.stringify({ choices: [{ delta: { content } }] }) +
      "\n\ndata: " +
      JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }] }) +
      "\n\ndata: [DONE]\n\n",
  );
});
model.listen(0, "127.0.0.1");
await once(model, "listening");
try {
  await mkdir(path.join(home, "data/user/settings"), { recursive: true });
  await writeFile(
    path.join(home, "data/user/settings/model_catalog.json"),
    JSON.stringify({
      version: 1,
      connections: [],
      services: {
        llm: {
          active_profile_id: "qa",
          active_model_id: "m",
          profiles: [
            {
              id: "qa",
              name: "Synthetic reading tutor",
              binding: "custom",
              api_key: "synthetic-only",
              base_url: `http://127.0.0.1:${model.address().port}/v1`,
              api_format: "chat_completions",
              models: [
                {
                  id: "m",
                  name: "Synthetic model",
                  model: "native-reading-test",
                  context_window: 200000,
                  max_tokens: 2000,
                },
              ],
            },
          ],
        },
      },
    }),
  );
  const p = await freePort();
  origin = `http://127.0.0.1:${p}`;
  backend = spawn(binary, ["--home", home, "serve", "--port", String(p)], {
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
  const scope = (
    await req("/api/settings/workspace/registrations", {
      name: "Reading tutor QA",
    })
  ).workspace.workspace_id;
  const upload = new FormData();
  upload.set("name", "Tutor source");
  upload.append(
    "files",
    new Blob([
      "Energy is conserved.\nSECOND_LINE_NOT_SELECTED\nEnd of the synthetic document.",
    ]),
    "source.md",
  );
  let response = await fetch(
    origin + `/api/knowledge-bases?dt_workspace=${scope}`,
    { method: "POST", body: upload },
  );
  assert.equal(response.status, 200, await response.text());
  const file = (
    await req(`/api/knowledge-bases/Tutor%20source/files?dt_workspace=${scope}`)
  ).files[0];
  const item = await req(`/api/reading/items?dt_workspace=${scope}`, {
    kb: "Tutor source",
    path: file.name,
    hash: file.sha256,
  });
  const body = {
    protocol_version: "2.0",
    capability: "reading_tutor",
    client_submission_id: "tutor-once",
    content: "Explain this selected sentence.",
    reading_target: {
      item_id: item.id,
      source_id: item.source_id,
      start_line: 1,
      end_line: 1,
    },
  };
  const receipt = await req(`/api/turns?dt_workspace=${scope}`, body);
  assert.equal(
    (await req(`/api/turns?dt_workspace=${scope}`, body)).turn_id,
    receipt.turn_id,
  );
  await until(async () => {
    const events = (
      await req(`/api/turns/${receipt.turn_id}/events?dt_workspace=${scope}`)
    ).events;
    return events.some(
      (e) => e.type === "done" && e.metadata.status === "completed",
    );
  });
  assert.equal(requests.length, 1);
  const system = requests[0].messages
    .filter((m) => ["system", "developer"].includes(m.role))
    .map((m) => m.content)
    .join("\n");
  assert.ok(system.includes('"line":1,"text":"Energy is conserved."'));
  assert.ok(!system.includes("SECOND_LINE_NOT_SELECTED"));
  assert.ok(system.includes(item.source_id));
  assert.ok(system.includes("untrusted reference data"));
  assert.ok(
    !requests[0].tools.some((t) =>
      /memory|notebook|study_publish|browser/.test(
        t.function?.name || t.name || "",
      ),
    ),
  );
  await req(
    `/api/turns?dt_workspace=other`,
    { ...body, client_submission_id: "cross" },
    404,
  );
  await req(
    `/api/turns?dt_workspace=${scope}`,
    {
      ...body,
      client_submission_id: "stale",
      reading_target: { ...body.reading_target, source_id: "wrong" },
    },
    409,
  );
  await req(
    `/api/turns?dt_workspace=${scope}`,
    { ...body, capability: "chat", client_submission_id: "wrong-mode" },
    400,
  );
  checks.push(
    "Immutable numbered reading context, source and workspace validation, bounded tool surface, and submission idempotency",
  );
  for (const url of [
    "http://127.0.0.1/",
    "http://169.254.169.254/",
    "http://[::1]/",
    "file:///etc/passwd",
  ])
    await req(
      `/api/reading/items/web?dt_workspace=${scope}`,
      { url, command_id: crypto.randomUUID() },
      400,
    );
  checks.push(
    "Web imports reject local/private addresses and non-web schemes before network access",
  );
  let webItem;
  if (process.env.LIVE_WEB_QA === "1") {
    webItem = await req(`/api/reading/items/web?dt_workspace=${scope}`, {
      url: "https://example.com/",
      command_id: "public-example",
    });
    assert.equal(webItem.kind, "web");
    assert.match(webItem.title, /Example Domain/);
    assert.equal(webItem.source_url, "https://example.com/");
    assert.match(
      (
        await req(
          `/api/reading/items/${webItem.id}/source?dt_workspace=${scope}`,
        )
      ).content,
      /documentation examples/i,
    );
    assert.equal(
      (
        await req(`/api/reading/items/web?dt_workspace=${scope}`, {
          url: "https://example.com/",
          command_id: "public-example",
        })
      ).id,
      webItem.id,
    );
    checks.push(
      "Real public HTTPS fetch, inert HTML extraction, immutable web snapshot and committed retry receipt",
    );
  }
  if (process.env.WEB_QA === "1") {
    const port = await freePort();
    const web = path.join(
      repo,
      "web",
      process.env.DEEPTUTOR_NEXT_DIST_DIR || ".next-import",
      "standalone",
    );
    const front = `http://127.0.0.1:${port}`;
    frontend = spawn(process.execPath, [path.join(web, "server.js")], {
      cwd: web,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        HOSTNAME: "127.0.0.1",
        PORT: String(port),
        DEEPTUTOR_API_BASE_URL: origin,
        BACKEND_PORT: String(p),
        DEEPTUTOR_HOME: home,
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
    browser = await chromium.launch({
      headless: true,
      ...(process.env.PLAYWRIGHT_CHANNEL
        ? { channel: process.env.PLAYWRIGHT_CHANNEL }
        : {}),
    });
    const page = await browser.newPage({
      viewport: { width: 1440, height: 1000 },
    });
    activePage = page;
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(
      front + `/learning/reading/${item.id}?dt_workspace=${scope}`,
    );
    await page
      .getByRole("button", { name: "New annotation", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Reading tutor", exact: true })
      .click();
    await page
      .getByRole("textbox", { name: /^Question about this passage/ })
      .fill("Explain this sentence in the UI.");
    let interrupted = false;
    await page.route("**/api/turns?*", async (route) => {
      if (!interrupted && route.request().method() === "POST") {
        interrupted = true;
        await route.fetch();
        await route.abort("failed");
      } else await route.continue();
    });
    const before = requests.length;
    await page.getByRole("button", { name: "Ask tutor", exact: true }).click();
    await page
      .getByRole("alert")
      .filter({ hasText: /fetch|Failed|Network|network|Request/i })
      .first()
      .waitFor();
    await page.unroute("**/api/turns?*");
    await page.reload();
    await page
      .getByRole("button", { name: "Reading tutor", exact: true })
      .click();
    await page.getByRole("button", { name: "Ask tutor", exact: true }).click();
    const panel = page.getByRole("region", {
      name: "Reading tutor",
      exact: true,
    });
    await panel
      .getByText("The selected source says energy is conserved.", {
        exact: false,
      })
      .waitFor();
    assert.equal(requests.length, before + 1);
    assert.equal(
      await panel
        .getByText("Explain this sentence in the UI.", { exact: true })
        .count(),
      1,
    );
    await panel
      .getByRole("link", { name: "Open conversation", exact: true })
      .waitFor();
    await page.screenshot({
      path: path.join(artifacts, "tutor-desktop.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForFunction(() => {
      const drawer = document.querySelector("[inert]");
      return drawer && drawer.getBoundingClientRect().right <= 1;
    });
    await page.screenshot({
      path: path.join(artifacts, "tutor-mobile.png"),
      fullPage: true,
    });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
    );
    await panel
      .getByRole("textbox", { name: /^Question about this passage/ })
      .scrollIntoViewIfNeeded();
    await page.screenshot({
      path: path.join(artifacts, "tutor-mobile-form.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await panel
      .getByRole("textbox", { name: /^Question about this passage/ })
      .fill("Please wait for cancellation.");
    await panel.getByRole("button", { name: "Ask tutor", exact: true }).click();
    await panel
      .getByRole("button", { name: "Stop response", exact: true })
      .click();
    await panel
      .getByRole("button", { name: "Stop response", exact: true })
      .waitFor({ state: "hidden" });
    checks.push(
      "Production tutor UI, frozen selected passage, lost-ack reload/retry without duplicate model calls, persisted transcript, cancellation, desktop/mobile layout",
    );
    if (webItem) {
      await page.goto(front + `/learning/reading?dt_workspace=${scope}`);
      await page
        .getByRole("textbox", { name: "Web page URL", exact: true })
        .fill("https://example.com/");
      await page
        .getByRole("button", { name: "Save web page", exact: true })
        .click();
      await page.waitForURL(new RegExp(`/learning/reading/${webItem.id}`));
      await page.getByText("Example Domain", { exact: true }).first().waitFor();
      checks.push(
        "Production URL import reopens the existing public web snapshot",
      );
    }
    assert.deepEqual(errors, []);
    await browser.close();
    browser = undefined;
    await stop(frontend);
    frontend = undefined;
  }
  const result = {
    passed: true,
    provider: "local synthetic Chat Completions SSE",
    real_public_web: process.env.LIVE_WEB_QA === "1",
    web_ui: process.env.WEB_QA === "1",
    model_calls: requests.length,
    checks,
  };
  await writeFile(
    path.join(artifacts, "result.json"),
    JSON.stringify(result, null, 2),
  );
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  if (activePage) {
    await activePage
      .screenshot({ path: path.join(artifacts, "failure.png"), fullPage: true })
      .catch(() => {});
    await writeFile(
      path.join(artifacts, "failure-page.txt"),
      activePage.url() +
        "\n" +
        (await activePage
          .locator("body")
          .innerText()
          .catch(() => "")),
    );
  }
  await writeFile(
    path.join(artifacts, "failure.log"),
    String(error) + "\n" + logs.slice(-10000),
  );
  throw error;
} finally {
  await browser?.close();
  await stop(frontend);
  await stop(backend);
  model.closeAllConnections();
  await new Promise((r) => model.close(r));
  await rm(temp, { recursive: true, force: true });
}
