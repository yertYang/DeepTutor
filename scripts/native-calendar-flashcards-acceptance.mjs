// Native HTTP + production UI acceptance using only a disposable learner profile.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { createServer } from "node:net";
import { mkdtemp, mkdir, rm, writeFile, readFile } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
const repo = fileURLToPath(new URL("../", import.meta.url));
const binary = path.resolve(
  process.env.NATIVE_TEST_BINARY ||
    "target/debug/deeptutor-server" +
      (process.platform === "win32" ? ".exe" : ""),
);
const temp = await mkdtemp(
  path.join(os.tmpdir(), "lorevia-calendar-flashcards-"),
);
const artifacts = path.join(repo, "dist/native-calendar-flashcards-acceptance");
await mkdir(artifacts, { recursive: true });
const exec = promisify(execFile);
let backend,
  frontend,
  browser,
  activePage,
  origin,
  logs = "";
const checks = [],
  shutdowns = [];
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
    let forced = false;
    c.kill("SIGTERM");
    const timer = setTimeout(
      () => {
        if (c.exitCode === null) {
          forced = true;
          c.kill("SIGKILL");
        }
      },
      c.spawnfile === binary ? 15000 : 5000,
    );
    try {
      await done;
    } finally {
      clearTimeout(timer);
    }
    shutdowns.push({
      service: c.spawnfile === binary ? "native" : "web",
      forced,
      exit_code: c.exitCode,
      signal: c.signalCode,
    });
    if (c.spawnfile === binary && process.platform !== "win32")
      assert.equal(
        forced,
        false,
        "Native server must drain streams and exit on SIGTERM",
      );
  }
};
const until = async (fn) => {
  for (let i = 0; i < 300; i++) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error("Timed out. " + logs.slice(-5000));
};
const launch = async (home) => {
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
};
const req = async (url, body, method = "POST", status = 200, headers = {}) => {
  const r = await fetch(
    origin + url,
    body === undefined
      ? {}
      : {
          method,
          headers: { "content-type": "application/json", ...headers },
          body: JSON.stringify(body),
        },
  );
  const v = await r.json();
  assert.equal(r.status, status, JSON.stringify(v));
  return v;
};
const command = async (url, body, method = "POST", status = 200) => {
  const current = await req("/api/calendar/calendars");
  return req(url, body, method, status, {
    "if-match": `"${current.revision}"`,
    "idempotency-key": crypto.randomUUID(),
  });
};
try {
  const home = path.join(temp, "home");
  await launch(home);
  const scope = (
    await req("/api/settings/workspace/registrations", {
      name: "Calendar and flashcard QA",
    })
  ).workspace.workspace_id;
  const task = (
    await command(`/api/task-board/cards?dt_workspace=${scope}`, {
      title: "Study source",
      due_date: "2026-11-04",
    })
  ).cards[0];
  assert.deepEqual((await req("/api/task-board?dt_workspace=other")).cards, []);
  const event = (
    await command("/api/calendar/events", {
      title: "DST regression",
      start: "2026-10-25T13:00:00Z",
      end: "2026-10-25T14:00:00Z",
      tzid: "America/New_York",
      rrule: "FREQ=WEEKLY;COUNT=5",
      reminders: [15],
      task_id: task.id,
      task_workspace_id: scope,
    })
  ).event;
  const range =
    "/api/calendar/events?start=2026-10-01T00:00:00Z&end=2026-12-01T00:00:00Z&tz=America%2FNew_York";
  const dates = (await req(range)).occurrences;
  assert.equal(dates.length, 5);
  assert.equal(dates[1].start, "2026-11-01T14:00:00.000Z");
  assert.equal(dates[1].reminder_times[0], "2026-11-01T13:45:00.000Z");
  const deleted = await command(
    `/api/calendar/events/${event.id}?scope=this&occurrence=${encodeURIComponent(dates[1].occurrence)}`,
    {},
    "DELETE",
  );
  assert.equal((await req(range)).occurrences.length, 4);
  await command("/api/calendar/undo/" + deleted.undo_token, {});
  assert.equal((await req(range)).occurrences.length, 5);
  checks.push(
    "DST recurrence, reminder instants, delete/undo and workspace-isolated tasks",
  );
  const note = await req(`/api/documents?dt_workspace=${scope}`, {
    title: "Flashcard acceptance",
    content: "A synthetic learning note.",
  });
  let deck = await req(
    `/api/documents/${note.id}/flashcards?dt_workspace=${scope}`,
  );
  assert.equal(deck.revision, 0);
  deck = await req(
    `/api/documents/${note.id}/flashcards?dt_workspace=${scope}`,
    {
      expected_revision: 0,
      command_id: "seed",
      cards: [{ id: "one", front: "What is conserved?", back: "Energy" }],
    },
    "PUT",
  );
  const grade = {
    expected_revision: deck.revision,
    command_id: "review-once",
    card_id: "one",
    remembered: true,
  };
  const reviewed = await req(
    `/api/documents/${note.id}/review?dt_workspace=${scope}`,
    grade,
  );
  assert.equal(reviewed.cards[0].reviews, 1);
  assert.equal(reviewed.cards[0].interval_days, 1);
  assert.equal(
    (await req(`/api/documents/${note.id}/review?dt_workspace=${scope}`, grade))
      .cards[0].reviews,
    1,
  );
  assert.deepEqual(
    (await req(`/api/documents/review/due?dt_workspace=${scope}`)).cards,
    [],
  );
  await req(
    `/api/documents/${note.id}/flashcards?dt_workspace=other`,
    undefined,
    "GET",
    404,
  );
  checks.push(
    "Authored flashcards, scheduled review, idempotent grades and scope isolation",
  );
  if (process.env.WEB_QA === "1") {
    const p = await freePort();
    const web = path.join(
      repo,
      "web",
      process.env.DEEPTUTOR_NEXT_DIST_DIR || ".next-import",
      "standalone",
    );
    const webOrigin = `http://127.0.0.1:${p}`;
    frontend = spawn(process.execPath, [path.join(web, "server.js")], {
      cwd: web,
      stdio: ["ignore", "pipe", "pipe"],
      env: {
        ...process.env,
        HOSTNAME: "127.0.0.1",
        PORT: String(p),
        DEEPTUTOR_API_BASE_URL: origin,
        BACKEND_PORT: new URL(origin).port,
        DEEPTUTOR_HOME: home,
        NEXT_TELEMETRY_DISABLED: "1",
      },
    });
    for (const s of [frontend.stdout, frontend.stderr])
      s.on("data", (b) => (logs += b));
    await until(async () => {
      try {
        return (await fetch(webOrigin + "/api/native/status")).ok;
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
      timezoneId: "America/New_York",
      locale: "en-US",
    });
    activePage = page;
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(webOrigin + `/calendar?dt_workspace=${scope}`);
    await page.getByRole("button", { name: "New event", exact: true }).click();
    await page
      .getByLabel("Event title", { exact: true })
      .fill("UI weekly study");
    await page.getByLabel("Repeat", { exact: true }).selectOption("weekly");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await page
      .getByLabel("Event title", { exact: true })
      .waitFor({ state: "hidden" });
    await until(
      async () =>
        (await req("/api/calendar/search?q=UI%20weekly")).events.length === 1,
    );
    await page.screenshot({
      path: path.join(artifacts, "calendar-desktop.png"),
      fullPage: true,
    });
    await page.reload();
    await page
      .getByRole("button", { name: "New event", exact: true })
      .waitFor();
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: path.join(artifacts, "calendar-mobile.png"),
      fullPage: true,
    });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth + 1,
      ),
    );
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(webOrigin + `/co-writer/${note.id}?dt_workspace=${scope}`);
    await page.getByRole("button", { name: "Flashcards", exact: true }).click();
    await page.getByRole("button", { name: "Add card", exact: true }).click();
    const panel = page.getByRole("region", { name: "Flashcards", exact: true });
    await panel.getByRole("textbox", { name: /^Front 2/ }).fill("UI question");
    await panel.getByRole("textbox", { name: /^Back 2/ }).fill("UI answer");
    await panel
      .getByRole("button", { name: "Save flashcards", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Show answer", exact: true })
      .waitFor();
    await page
      .getByRole("button", { name: "Show answer", exact: true })
      .click();
    await page.getByRole("button", { name: "Remembered", exact: true }).click();
    await page
      .getByText(
        "No flashcards due. Come back when the next review is scheduled.",
        { exact: true },
      )
      .waitFor();
    const downloadEvent = page.waitForEvent("download");
    await page
      .getByRole("link", { name: "Export flashcards (Anki)", exact: true })
      .click();
    const download = await downloadEvent;
    assert.ok(
      (await readFile(await download.path(), "utf8")).includes(
        "UI question\tUI answer",
      ),
    );
    await page.screenshot({
      path: path.join(artifacts, "flashcards-desktop.png"),
      fullPage: true,
    });
    await page.reload();
    await page.getByRole("button", { name: "Flashcards", exact: true }).click();
    await panel.getByRole("textbox", { name: /^Front 2/ }).waitFor();
    assert.equal(
      await panel.getByRole("textbox", { name: /^Back 2/ }).inputValue(),
      "UI answer",
    );
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: path.join(artifacts, "flashcards-mobile.png"),
      fullPage: true,
    });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth + 1,
      ),
    );
    assert.deepEqual(errors, []);
    checks.push(
      "Production calendar create/repeat/save/reload and responsive layout",
      "Production flashcard edit/review/Anki download/reload and responsive layout",
    );
    await browser.close();
    browser = undefined;
    await stop(frontend);
    frontend = undefined;
  }
  await stop(backend);
  backend = undefined;
  const backup = path.join(temp, "backup"),
    restored = path.join(temp, "restored");
  await exec(binary, ["--home", home, "backup", "--output", backup]);
  await exec(binary, ["--home", restored, "restore", "--from", backup]);
  await launch(restored);
  assert.ok(
    (await req(range)).occurrences.some((v) => v.event_id === event.id),
  );
  assert.equal(
    (await req(`/api/documents/${note.id}/flashcards?dt_workspace=${scope}`))
      .cards[0].reviews,
    1,
  );
  assert.equal(
    (await req(`/api/task-board?dt_workspace=${scope}`)).cards[0].id,
    task.id,
  );
  checks.push(
    "Calendar, task cards and flashcard schedules survive complete backup/restore",
  );
  const result = {
    passed: true,
    web: process.env.WEB_QA === "1",
    checks,
    shutdowns,
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
    String(error) + "\n" + logs.slice(-12000),
  );
  throw error;
} finally {
  await browser?.close();
  await stop(frontend);
  await stop(backend);
  await rm(temp, { recursive: true, force: true });
}
