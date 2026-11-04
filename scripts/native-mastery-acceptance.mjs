// Native model-tool workflow with a synthetic provider and disposable home.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { createServer as httpServer } from "node:http";
import { createServer } from "node:net";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
const repo = fileURLToPath(new URL("../", import.meta.url));
const exec = promisify(execFile);
const binary = path.resolve(
  process.env.NATIVE_TEST_BINARY ||
    "target/debug/deeptutor-server" +
      (process.platform === "win32" ? ".exe" : ""),
);
const temp = await mkdtemp(path.join(os.tmpdir(), "lorevia-mastery-"));
const home = path.join(temp, "home");
const artifacts = path.join(repo, "dist/native-mastery-acceptance");
await mkdir(artifacts, { recursive: true });
let backend,
  frontend,
  browser,
  activePage,
  origin,
  logs = "",
  calls = 0,
  round = 0;
const checks = [];
const freePort = async () => {
  const s = createServer().listen(0, "127.0.0.1");
  await once(s, "listening");
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const until = async (fn) => {
  for (let i = 0; i < 300; i++) {
    const value = await fn();
    if (value) return value;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error("Timed out. " + logs.slice(-4000));
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
const req = async (url, body, status = 200) => {
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
  assert.equal(r.status, status, JSON.stringify(v));
  return v;
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
const model = httpServer(async (request, response) => {
  try {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    const body = JSON.parse(raw);
    calls++;
    const system =
      body.messages.find((m) => m.role === "system")?.content || "";
    const results = body.messages
      .filter((m) => m.role === "tool")
      .map((m) => {
        try {
          return JSON.parse(m.content);
        } catch {
          return null;
        }
      })
      .filter(Boolean);
    const committed = results
      .filter((r) => r.content?.kind && r.content?.phase)
      .at(-1)?.content;
    const questions = system.includes(
      "Generate a usable practice question set",
    );
    let name, args;
    if (!committed) {
      name = "study_plan";
      if (questions) round++;
      args = {
        title: questions
          ? `Synthetic questions ${round}`
          : "Synthetic mastery path",
        objectives: ["Understand the foundation", "Apply the idea"],
      };
    } else if (committed.phase !== "published") {
      name = "study_publish";
      args = questions
        ? {
            questions: [1, 2, 3].map((i) => ({
              prompt: `${committed.title} question ${i}?`,
              choices: ["Correct", "Wrong"],
              answer_index: 0,
              explanation: "Synthetic explanation",
            })),
          }
        : {
            units: [
              {
                title: "Foundation",
                goals: ["Explain the basics"],
                prerequisites: [],
              },
              {
                title: "Application",
                goals: ["Use the basics"],
                prerequisites: [0],
              },
            ],
          };
    }
    const delta = name
      ? {
          tool_calls: [
            {
              index: 0,
              id: `call-${calls}`,
              type: "function",
              function: { name, arguments: JSON.stringify(args) },
            },
          ],
        }
      : { content: "Saved the study artifact." };
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write(
      "data: " +
        JSON.stringify({
          choices: [{ index: 0, delta, finish_reason: null }],
        }) +
        "\n\n",
    );
    response.end(
      "data: " +
        JSON.stringify({
          choices: [
            {
              index: 0,
              delta: {},
              finish_reason: name ? "tool_calls" : "stop",
            },
          ],
        }) +
        "\n\ndata: [DONE]\n\n",
    );
  } catch (error) {
    response.writeHead(500);
    response.end(String(error));
  }
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
              name: "Synthetic mastery model",
              binding: "custom",
              api_key: "synthetic-only",
              base_url: `http://127.0.0.1:${model.address().port}/v1`,
              api_format: "chat_completions",
              models: [
                {
                  id: "m",
                  name: "Synthetic model",
                  model: "native-mastery-test",
                  context_window: 200000,
                  max_tokens: 4000,
                },
              ],
            },
          ],
        },
      },
    }),
  );
  await launch(home);
  const scope = (
    await req("/api/settings/workspace/registrations", { name: "Mastery QA" })
  ).workspace.workspace_id;
  const complete = async (id) => {
    await until(async () => {
      const state = await req(`/api/study?dt_workspace=${scope}`);
      const t = state.turns.find((t) => t.id === id);
      if (!t || ["queued", "running", "waiting_input"].includes(t.status))
        return false;
      assert.equal(t.status, "completed", t.error);
      return true;
    });
    return req(`/api/study/${id}?dt_workspace=${scope}`);
  };
  const start = async (capability, study_target) => {
    const result = await req(`/api/turns?dt_workspace=${scope}`, {
      protocol_version: "2.0",
      capability,
      content: "Build this synthetic learning activity.",
      client_submission_id: crypto.randomUUID(),
      ...(study_target ? { study_target } : {}),
    });
    return complete(result.turn_id);
  };
  const plan = await start("mastery_path");
  assert.equal(plan.result.units.length, 2);
  let progress = await req(`/api/study/progress?dt_workspace=${scope}`);
  assert.equal(progress.objectives[1].status, "locked");
  await req(
    `/api/turns?dt_workspace=${scope}`,
    {
      capability: "deep_question",
      content: "Premature objective",
      study_target: { plan_id: plan.id, objective_id: "O2" },
      client_submission_id: "blocked",
    },
    400,
  );
  const quiz = await start("deep_question", {
    plan_id: plan.id,
    objective_id: "O1",
  });
  assert.equal(quiz.result.questions[0].answer_index, undefined);
  assert.ok(
    quiz.result.questions.every(
      (q) => q.plan_id === plan.id && q.objective_id === "O1",
    ),
  );
  const answer = async (q, id = crypto.randomUUID()) =>
    req(`/api/study/${q.id}/attempts?dt_workspace=${scope}`, {
      command_id: id,
      expected_revision: q.revision,
      question_id: "Q1",
      answer_index: 0,
    });
  for (const question of quiz.result.questions) {
    const input = {
      command_id: crypto.randomUUID(),
      expected_revision: quiz.revision,
      question_id: question.id,
      answer_index: 0,
    };
    const first = await req(
      `/api/study/${quiz.id}/attempts?dt_workspace=${scope}`,
      input,
    );
    assert.deepEqual(
      await req(`/api/study/${quiz.id}/attempts?dt_workspace=${scope}`, input),
      first,
    );
  }
  progress = await req(`/api/study/progress?dt_workspace=${scope}`);
  assert.equal(progress.objectives[0].status, "mastered");
  assert.equal(progress.objectives[0].correct_questions, 3);
  assert.equal(progress.objectives[1].status, "ready");
  const firstDue = progress.objectives[0].review_due_at;
  await answer(quiz);
  assert.equal(
    (await req(`/api/study/progress?dt_workspace=${scope}`)).objectives[0]
      .review_due_at,
    firstDue,
  );
  const advanced = await start("deep_question", {
    plan_id: plan.id,
    objective_id: "O2",
  });
  for (const question of advanced.result.questions)
    await req(`/api/study/${advanced.id}/attempts?dt_workspace=${scope}`, {
      command_id: crypto.randomUUID(),
      expected_revision: advanced.revision,
      question_id: question.id,
      answer_index: 0,
    });
  assert.ok(
    (await req(`/api/study/progress?dt_workspace=${scope}`)).objectives.every(
      (o) => o.status === "mastered",
    ),
  );
  assert.equal(
    (await req(`/api/study/reviews?dt_workspace=${scope}`)).upcoming.length,
    2,
  );
  assert.deepEqual(
    (await req("/api/study/reviews?dt_workspace=other")).upcoming,
    [],
  );
  checks.push(
    "Model-generated acyclic path, locked prerequisites, three fresh first-exposure answers, idempotent attempts, repeat-answer fencing and bound objective progression",
  );
  if (process.env.WEB_QA === "1") {
    const p = await freePort(),
      front = `http://127.0.0.1:${p}`,
      web = path.join(
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
    // A deep link must load its requested path even when the paginated list omits it.
    await page.route(/\/api\/study\?/, async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      body.artifacts = body.artifacts.filter((a) => a.id !== plan.id);
      await route.fulfill({ response, json: body });
    });
    await page.goto(
      front + `/learning/mastery/${plan.id}?dt_workspace=${scope}`,
    );
    await page
      .getByRole("button", { name: "Foundation: Round completed", exact: true })
      .waitFor();
    await page
      .getByRole("region", { name: "Review queue", exact: true })
      .getByText("No objectives are due for review.", { exact: true })
      .waitFor();
    await page
      .getByRole("button", { name: "Foundation: Round completed", exact: true })
      .focus();
    await page.keyboard.press("Enter");
    await page.screenshot({
      path: path.join(artifacts, "mastery-desktop.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForFunction(() => {
      const drawer = document.querySelector("[inert]");
      return drawer && drawer.getBoundingClientRect().right <= 1;
    });
    await page.screenshot({
      path: path.join(artifacts, "mastery-mobile.png"),
      fullPage: true,
    });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
    );
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.unroute(/\/api\/study\?/);
    await page
      .getByRole("button", { name: "Practice this objective", exact: true })
      .first()
      .click();
    assert.equal(
      await page.getByLabel("Study mode", { exact: true }).inputValue(),
      "deep_question",
    );
    await page
      .getByRole("button", { name: "Start study", exact: true })
      .click();
    const forms = page.locator("form").filter({
      has: page.getByRole("button", { name: "Check answer", exact: true }),
    });
    await until(async () => (await forms.count()) === 3);
    for (let i = 0; i < 3; i++) {
      await forms.nth(i).getByRole("radio").first().check();
      await forms
        .nth(i)
        .getByRole("button", { name: "Check answer", exact: true })
        .click();
      await forms.nth(i).getByRole("status").locator("strong").waitFor();
      assert.equal(
        await forms.nth(i).getByRole("status").locator("strong").innerText(),
        "Correct",
      );
    }
    assert.equal(
      (await req(`/api/study/progress?dt_workspace=${scope}`)).objectives[0]
        .review_due_at,
      firstDue,
    );
    const graded = await req(`/api/study/progress?dt_workspace=${scope}`);
    assert.equal(graded.objectives[0].correct_questions, 6);
    assert.equal(graded.objectives[0].review_stage, 1);
    assert.deepEqual(errors, []);
    checks.push(
      "Production deep-linked mastery map, omitted-list recovery, keyboard navigation, review queue, objective-bound question generation/answer feedback and mobile layout",
    );
    await browser.close();
    browser = undefined;
    await stop(frontend);
    frontend = undefined;
  }
  const before = await req(`/api/study/progress?dt_workspace=${scope}`);
  assert.equal(before.total, process.env.WEB_QA === "1" ? 10 : 7);
  await stop(backend);
  backend = undefined;
  const backup = path.join(temp, "backup"),
    restored = path.join(temp, "restored");
  await exec(binary, ["--home", home, "backup", "--output", backup]);
  await exec(binary, ["--home", restored, "restore", "--from", backup]);
  await launch(restored);
  assert.deepEqual(
    (await req(`/api/study/progress?dt_workspace=${scope}`)).objectives,
    before.objectives,
  );
  checks.push(
    "Prerequisite states and precise review due dates survive complete backup and restore",
  );
  const result = {
    passed: true,
    provider: "local synthetic Chat Completions SSE",
    web: process.env.WEB_QA === "1",
    model_calls: calls,
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
