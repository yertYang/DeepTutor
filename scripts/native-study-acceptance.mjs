// Run after cargo build -p deeptutor-server --bin deeptutor-server.
// WEB_QA=1 also checks the production Web build and requires packages/browser-testing dependencies.
// Every provider response and learner document is synthetic; the disposable home is removed on exit.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createServer as tcpServer } from "node:net";
import { once } from "node:events";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import {
  mkdtemp,
  mkdir,
  writeFile,
  rm,
  realpath,
  readFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
const exec = promisify(execFile),
  repo = fileURLToPath(new URL("../", import.meta.url));
const temp = await mkdtemp(path.join(os.tmpdir(), "native-study-qa-")),
  home = path.join(temp, "home"),
  binary = path.resolve(
    process.env.NATIVE_TEST_BINARY ||
      path.join(
        repo,
        "target/debug/deeptutor-server" +
          (process.platform === "win32" ? ".exe" : ""),
      ),
  );
const artifacts = path.join(repo, "dist/native-study-acceptance");
await mkdir(artifacts, { recursive: true });
let backend,
  frontend,
  browser,
  origin,
  logs = "",
  calls = 0;
const systemPrompts = [];
const stop = async (child, signal = "SIGTERM") => {
  if (child && child.exitCode === null) {
    const done = once(child, "exit");
    child.kill(signal);
    await done;
  }
};
const until = async (fn, timeout = 30000) => {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const v = await fn();
    if (v) return v;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("Timed out: " + logs.slice(-5000));
};
const freePort = async () => {
  const s = tcpServer();
  s.listen(0, "127.0.0.1");
  await once(s, "listening");
  const p = s.address().port;
  await new Promise((r) => s.close(r));
  return p;
};
const model = createServer(async (req, res) => {
  try {
    let raw = "";
    for await (const b of req) raw += b;
    const body = JSON.parse(raw);
    calls++;
    const system =
      body.messages.find((m) => m.role === "system")?.content || "";
    systemPrompts.push(system);
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
    const research = system.includes(
      "Conduct a source-grounded research workflow",
    );
    const questions = system.includes(
      "Generate a usable practice question set",
    );
    const noteAi = system.includes("Create a reviewable edit proposal"),
      memory = system.includes("Propose a concise reorganization");
    let name, args;
    if (!committed) {
      name = "study_plan";
      args = {
        title: research
          ? "合成研究报告"
          : questions
            ? "合成练习题"
            : noteAi
              ? "合成笔记提案"
              : memory
                ? "合成记忆提案"
                : "合成学习路径",
        objectives: ["理解能量守恒"],
      };
    } else if (research) {
      const rag = results.find((r) => r.content?.chunks);
      if (!rag) {
        name = "rag";
        args = { query: "能量", kb_name: "教材" };
      } else if (!committed.evidence.length) {
        const hit = rag.content.chunks[0];
        name = "study_evidence";
        args = {
          kb_name: "教材",
          path: hit.source_path,
          hash: hit.source_hash,
          start_line: hit.start_line,
          end_line: hit.end_line,
        };
      } else if (committed.phase !== "published") {
        name = "study_publish";
        args = {
          report: "## 研究结论\n能量守恒，有真实来源支持 [E1]。",
          citations: ["E1"],
        };
      }
    } else if (committed.phase !== "published") {
      name = "study_publish";
      args = noteAi
        ? {
            blocks: [
              {
                id: "generated-block",
                type: "paragraph",
                props: {},
                content: [
                  {
                    type: "text",
                    text: "清晰表达的能量守恒笔记。",
                    styles: {},
                  },
                ],
                children: [],
              },
            ],
          }
        : memory
          ? { content: "- 用户喜欢通过例子学习。" }
          : questions
            ? {
                questions: [
                  {
                    prompt: "能量会凭空消失吗？",
                    choices: ["不会", "会"],
                    answer_index: 0,
                    explanation: "能量守恒，能量可以转化但不会凭空消失。",
                  },
                ],
              }
            : {
                units: [
                  {
                    title: "能量基础",
                    goals: ["解释能量守恒"],
                    prerequisites: [],
                  },
                  {
                    title: "能量应用",
                    goals: ["完成能量转换练习"],
                    prerequisites: [0],
                  },
                ],
              };
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
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
      : { content: "学习成果已保存。" };
    res.write(
      `data: ${JSON.stringify({ id: `response-${calls}`, choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`,
    );
    res.write(
      `data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: name ? "tool_calls" : "stop" }], usage: { prompt_tokens: 20, completion_tokens: 10, total_tokens: 30 } })}\n\n`,
    );
    res.end("data: [DONE]\n\n");
  } catch (error) {
    res.writeHead(500);
    res.end(error.message);
  }
});
model.listen(0, "127.0.0.1");
await once(model, "listening");
const request = async (
  route,
  body,
  method = body === undefined ? "GET" : "POST",
  expected = 200,
) => {
  const response = await fetch(origin + route, {
    method,
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const v = await response.json();
  assert.equal(response.status, expected, JSON.stringify(v));
  return v;
};
const launch = async (root = home) => {
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
const completed = async (id, scope) => {
  await until(async () => {
    const state = await request("/api/study?workspace_id=" + scope);
    const turn = state.turns.find((t) => t.id === id);
    if (!turn) return false;
    if (!["running", "queued", "waiting_input"].includes(turn.status)) {
      assert.equal(turn.status, "completed", turn.error);
      return true;
    }
    return false;
  });
  return request(`/api/study/${id}?workspace_id=${scope}`);
};
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
              name: "Synthetic local provider",
              binding: "custom",
              api_key: "synthetic-only",
              base_url: `http://127.0.0.1:${model.address().port}/v1`,
              api_format: "chat_completions",
              models: [
                {
                  id: "m",
                  name: "Synthetic model",
                  model: "native-study-test",
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
  await launch();
  const workspace = (
    await request("/api/settings/workspace/registrations", {
      name: "物理学习",
      instructions: "学习时使用中文例子",
    })
  ).workspace;
  const scope = workspace.workspace_id;
  await request(
    `/api/settings/workspace/registrations/${scope}`,
    { name: "陈旧改名", expected_revision: 0 },
    "PATCH",
    409,
  );
  const body = new FormData();
  body.set("name", "教材");
  body.append(
    "files",
    new Blob(["能量守恒。\n能量不会凭空消失，而会转化为其他形式。"]),
    "physics.md",
  );
  let response = await fetch(
    origin + "/api/knowledge-bases?workspace_id=" + scope,
    { method: "POST", body },
  );
  assert.equal(response.status, 200, await response.text());
  await request(
    "/api/turns?workspace_id=" + scope,
    {
      capability: "deep_research",
      content: "研究能量守恒",
      client_submission_id: "no-source",
    },
    "POST",
    400,
  );
  assert.equal(calls, 0);
  const researchRequest = {
    capability: "deep_research",
    content: "研究能量守恒",
    knowledge_bases: ["教材"],
    client_submission_id: "research-request",
  };
  const research = await request(
    "/api/turns?workspace_id=" + scope,
    researchRequest,
  );
  const report = await completed(research.turn_id, scope);
  assert.equal(report.evidence[0].quote.includes("能量守恒"), true);
  assert.equal(report.phase, "published");
  const count = calls;
  const repeated = await request(
    "/api/turns?workspace_id=" + scope,
    researchRequest,
  );
  assert.equal(repeated.turn_id, research.turn_id);
  assert.equal(repeated.replay, true);
  assert.equal(calls, count);
  const pathTurn = await request("/api/turns?workspace_id=" + scope, {
    capability: "mastery_path",
    content: "制定能量学习路径",
    client_submission_id: "path-request",
  });
  const plan = await completed(pathTurn.turn_id, scope);
  assert.equal(plan.result.units.length, 2);
  const questionTurn = await request("/api/turns?workspace_id=" + scope, {
    capability: "deep_question",
    content: "为能量基础生成练习",
    study_target: { plan_id: plan.id, objective_id: "O1" },
    client_submission_id: "question-request",
  });
  const quiz = await completed(questionTurn.turn_id, scope);
  assert.equal(quiz.result.questions[0].answer_index, undefined);
  assert.equal(quiz.result.questions[0].plan_id, plan.id);
  const answer = {
    question_id: "Q1",
    answer_index: 0,
    expected_revision: quiz.revision,
    command_id: "answer-command",
  };
  const grade = await request(
    `/api/study/${quiz.id}/attempts?workspace_id=${scope}`,
    answer,
  );
  assert.equal(grade.correct, true);
  assert.deepEqual(
    await request(
      `/api/study/${quiz.id}/attempts?workspace_id=${scope}`,
      answer,
    ),
    grade,
  );
  await request(
    `/api/study/${quiz.id}/attempts?workspace_id=${scope}`,
    { ...answer, answer_index: 1 },
    "POST",
    409,
  );
  await request(
    `/api/study/${quiz.id}?workspace_id=other`,
    undefined,
    "GET",
    404,
  );
  const progress = await request("/api/study/progress?workspace_id=" + scope);
  assert.equal(progress.total, 1);
  assert.equal(progress.objectives[0].correct_questions, 1);
  const note = await request(
    `/api/study/${report.id}/note?workspace_id=${scope}`,
    { expected_revision: report.revision },
  );
  assert.match(note.content, /能量守恒/);

  const sourceNote = await request("/api/documents?workspace_id=" + scope, {
    title: "待编辑笔记",
    content: "能量守恒原文。",
  });
  const editTurn = await request("/api/turns?workspace_id=" + scope, {
    capability: "note_ai",
    content: "改善表达",
    document_target: {
      document_id: sourceNote.id,
      expected_revision: sourceNote.revision,
    },
    client_submission_id: "note-ai",
  });
  const edit = await completed(editTurn.turn_id, scope);
  assert.equal(
    (
      await request(
        "/api/documents/" + sourceNote.id + "?workspace_id=" + scope,
      )
    ).content,
    sourceNote.content,
  );
  const applied = await request(
    "/api/study/" + edit.id + "/apply?workspace_id=" + scope,
    {
      expected_revision: edit.revision,
      expected_target_revision: sourceNote.revision,
    },
  );
  assert.match(applied.content, /清晰表达/);
  await request(
    "/api/memory/doc/L3/preferences?workspace_id=" + scope,
    {
      content: "- 用户喜欢通过例子学习。\n- 用户喜欢通过例子学习。",
      expected_revision: 0,
    },
    "PUT",
  );
  const memory = await request(
    "/api/memory/doc/L3/preferences?workspace_id=" + scope,
  );
  const memoryTurn = await request("/api/turns?workspace_id=" + scope, {
    capability: "memory_consolidate",
    content: "去除重复",
    memory_target: {
      layer: "L3",
      key: "preferences",
      expected_revision: memory.revision,
    },
    client_submission_id: "memory-ai",
  });
  const memoryEdit = await completed(memoryTurn.turn_id, scope);
  assert.equal(
    (await request("/api/memory/doc/L3/preferences?workspace_id=" + scope))
      .content,
    memory.content,
  );

  const wordBytes = await readFile(
    path.join(repo, "rust/server/tests/fixtures/import.docx"),
  );
  const importWord = async () => {
    const form = new FormData();
    form.append("file", new Blob([wordBytes]), "讲义.docx");
    form.append("client_submission_id", "word-import-request");
    const response = await fetch(
      origin + "/api/documents/import/docx?workspace_id=" + scope,
      { method: "POST", body: form },
    );
    const value = await response.json();
    assert.equal(response.status, 200, JSON.stringify(value));
    return value;
  };
  const imported = await importWord();
  assert.match(imported.content, /能量守恒/);
  assert.equal((await importWord()).id, imported.id);
  await request(
    "/api/memory/doc/L2/chat?workspace_id=" + scope,
    { content: "- 重复条目\n- 重复条目", expected_revision: 0 },
    "PUT",
  );
  await request(
    "/api/memory/doc/L2/cowriter?workspace_id=" + scope,
    {
      content: "- 引用已导入的讲义 [^1]\n[^1]: cowriter:" + imported.id,
      expected_revision: 0,
    },
    "PUT",
  );
  const graph = await request("/api/memory/graph?workspace_id=" + scope);
  assert.equal(graph.l1.cowriter[0].id, imported.id);
  assert.equal(
    (await request("/api/memory/graph?workspace_id=other")).l1.cowriter.length,
    0,
  );
  const policy = await request("/api/memory/maintenance?workspace_id=" + scope);
  assert.equal(policy.enabled, false);
  await request(
    "/api/memory/maintenance?workspace_id=" + scope,
    { enabled: true, expected_revision: policy.revision },
    "PUT",
  );
  await request(
    "/api/memory/maintenance?workspace_id=" + scope,
    { enabled: false, expected_revision: 0 },
    "PUT",
    409,
  );
  const cleanup = await request(
    "/api/memory/maintenance/scan?workspace_id=" + scope,
    {},
  );
  assert.deepEqual(
    (await request("/api/memory/maintenance/scan?workspace_id=" + scope, {}))
      .proposal_ids,
    cleanup.proposal_ids,
  );
  const cleanupArtifacts = await Promise.all(
    cleanup.proposal_ids.map((id) =>
      request("/api/study/" + id + "?workspace_id=" + scope),
    ),
  );
  const cleanupChat = cleanupArtifacts.find(
    (value) =>
      value.proposal_target.layer === "L2" &&
      value.proposal_target.key === "chat",
  );
  assert.ok(cleanupChat);
  assert.equal(
    (await request("/api/memory/doc/L2/chat?workspace_id=" + scope)).revision,
    1,
  );
  const importPolicy = await request(
    "/api/knowledge-bases/supported-file-types",
  );
  if (importPolicy.ocr_available && process.env.OCR_QA === "1") {
    for (const [file, name] of [
      ["ocr.png", "ocr.png"],
      ["ocr.pdf", "scan.pdf"],
    ]) {
      const form = new FormData();
      form.append(
        "files",
        new Blob([
          await readFile(path.join(repo, "rust/server/tests/fixtures", file)),
        ]),
        name,
      );
      const response = await fetch(
        origin +
          "/api/knowledge-bases/" +
          encodeURIComponent("教材") +
          "/upload?workspace_id=" +
          scope,
        { method: "POST", body: form },
      );
      assert.equal(response.status, 200, await response.text());
    }
    const search = await request(
      "/api/knowledge-bases/" +
        encodeURIComponent("教材") +
        "/search?workspace_id=" +
        scope,
      { query: "ENERGY", top_k: 10 },
    );
    assert.ok(search.chunks.some((row) => row.source_path === "ocr.png"));
    assert.ok(search.chunks.some((row) => row.source_path === "scan.pdf"));
  }
  assert.ok(systemPrompts.every((p) => p.includes("学习时使用中文例子")));
  await request(
    `/api/settings/workspace/registrations/${scope}`,
    { archived: true, expected_revision: 1 },
    "PATCH",
  );
  await request(
    "/api/turns?workspace_id=" + scope,
    {
      capability: "deep_question",
      content: "归档后的新任务",
      client_submission_id: "archived-request",
    },
    "POST",
    409,
  );
  await request(
    `/api/settings/workspace/registrations/${scope}`,
    { archived: false, expected_revision: 2 },
    "PATCH",
  );
  if (process.env.WEB_QA === "1") {
    const p = await freePort(),
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
        DEEPTUTOR_HOME: home,
        DEEPTUTOR_API_BASE_URL: origin,
        BACKEND_PORT: new URL(origin).port,
        NEXT_TELEMETRY_DISABLED: "1",
      },
    });
    for (const s of [frontend.stdout, frontend.stderr])
      s.on("data", (b) => (logs += b));
    const front = `http://127.0.0.1:${p}`;
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
    const page = await browser.newPage({
      viewport: { width: 1440, height: 1000 },
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(front + "/settings/workspace?dt_workspace=" + scope);
    await page
      .getByRole("heading", { name: "物理学习", exact: true })
      .waitFor();
    await page
      .getByRole("textbox", { name: "Workspace name", exact: true })
      .first()
      .fill("界面创建工作区");
    await page
      .getByRole("button", { name: "Create workspace", exact: true })
      .click();
    await page
      .getByRole("heading", { name: "界面创建工作区", exact: true })
      .waitFor();
    await page.screenshot({
      path: path.join(artifacts, "workspaces.png"),
      fullPage: true,
    });
    await page.goto(front + "/learning/native?dt_workspace=" + scope);
    await page.getByRole("button", { name: /合成研究报告/ }).click();
    await page
      .getByText("能量守恒，有真实来源支持", { exact: false })
      .waitFor();
    assert.equal(
      await page
        .getByText("Some records could not be loaded.", { exact: false })
        .count(),
      0,
    );
    await page
      .getByRole("link", { name: /合成研究报告/ })
      .first()
      .waitFor();
    await page.screenshot({
      path: path.join(artifacts, "research.png"),
      fullPage: true,
    });
    await page.getByRole("button", { name: /合成练习题/ }).click();
    await page.getByText("能量会凭空消失吗？", { exact: true }).waitFor();
    await page.getByRole("radio").first().check();
    await page
      .getByRole("button", { name: "Check answer", exact: true })
      .click();
    await page
      .getByText("能量守恒，能量可以转化但不会凭空消失。", { exact: true })
      .waitFor();
    await page.reload();
    await page.getByRole("button", { name: /合成练习题/ }).click();
    await page
      .getByText("能量守恒，能量可以转化但不会凭空消失。", { exact: true })
      .waitFor();
    await page.screenshot({
      path: path.join(artifacts, "practice.png"),
      fullPage: true,
    });
    await page.getByRole("button", { name: /合成学习路径/ }).click();
    await page
      .getByRole("button", { name: "Practice this objective", exact: true })
      .first()
      .click();
    await page.getByText("Selected objective:", { exact: false }).waitFor();
    assert.equal(
      await page
        .getByRole("textbox", { name: "Learning goal", exact: true })
        .inputValue(),
      "能量基础\n解释能量守恒",
    );
    await page.setViewportSize({ width: 390, height: 664 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
      true,
    );
    await page.screenshot({
      path: path.join(artifacts, "study-mobile.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(
      front +
        "/learning/native?kind=memory_consolidate&memory_layer=L3&memory_key=preferences&revision=" +
        memory.revision +
        "&dt_workspace=" +
        scope,
    );
    await page.getByRole("button", { name: /合成记忆提案/ }).click();
    await page
      .getByText(
        "Review the proposal before applying it. The source stays unchanged until you apply.",
        { exact: true },
      )
      .waitFor();
    page.once("dialog", (d) => d.accept());
    await page
      .getByRole("button", { name: "Apply reviewed changes", exact: true })
      .click();
    await page.getByRole("button", { name: "Applied", exact: true }).waitFor();
    assert.equal(
      (await request("/api/memory/doc/L3/preferences?workspace_id=" + scope))
        .content,
      "- 用户喜欢通过例子学习。",
    );
    await page.screenshot({
      path: path.join(artifacts, "memory-proposal.png"),
      fullPage: true,
    });
    await page.getByRole("button", { name: /合成笔记提案/ }).click();
    await page.getByRole("button", { name: "Applied", exact: true }).waitFor();
    await page.screenshot({
      path: path.join(artifacts, "note-proposal.png"),
      fullPage: true,
    });
    await page.goto(front + "/memory?dt_workspace=" + scope);
    await page.getByText("Automatic memory cleanup", { exact: true }).click();
    await page
      .getByRole("button", { name: "Scan saved memory now", exact: true })
      .click();
    await page
      .getByRole("link", { name: "Review cleanup proposal", exact: true })
      .first()
      .waitFor();
    await page.getByText("用户喜欢通过例子学习。", { exact: false }).waitFor();
    await page.screenshot({
      path: path.join(artifacts, "memory-maintenance.png"),
      fullPage: true,
    });
    await page.goto(
      front +
        "/learning/native?artifact_id=" +
        cleanupChat.id +
        "&dt_workspace=" +
        scope,
    );
    await page
      .getByRole("heading", { name: "Memory cleanup · L2/chat", exact: true })
      .waitFor();
    page.once("dialog", (dialog) => dialog.accept());
    await page
      .getByRole("button", { name: "Apply reviewed changes", exact: true })
      .click();
    await page.getByRole("button", { name: "Applied", exact: true }).waitFor();
    assert.equal(
      (await request("/api/memory/doc/L2/chat?workspace_id=" + scope)).content,
      "- 重复条目",
    );
    await page.goto(front + "/memory/graph?dt_workspace=" + scope);
    await page
      .getByRole("heading", { name: "Memory graph", exact: true })
      .waitFor();
    const node = page.getByRole("button", { name: /L2 · 引用已导入/ });
    await node.focus();
    await page.keyboard.press("Enter");
    await page
      .getByRole("complementary", {
        name: "Selected memory entry",
        exact: true,
      })
      .waitFor();
    await page
      .getByRole("link", { name: "Open source", exact: true })
      .waitFor();
    await page.screenshot({
      path: path.join(artifacts, "memory-graph.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 664 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth + 1,
      ),
      true,
    );
    await page
      .getByRole("link", { name: "Open source", exact: true })
      .click({ trial: true });
    await page.waitForTimeout(500);
    await page.screenshot({
      path: path.join(artifacts, "memory-graph-mobile.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(front + "/co-writer?dt_workspace=" + scope);
    await page
      .getByRole("button", { name: "Import Word", exact: true })
      .first()
      .waitFor();
    await page
      .locator("input[type=file]")
      .setInputFiles(path.join(repo, "rust/server/tests/fixtures/import.docx"));
    await page
      .getByRole("button", { name: "All pages", exact: true })
      .waitFor();
    await page.getByText("Native imported lecture", { exact: false }).waitFor();
    await page.screenshot({
      path: path.join(artifacts, "word-import.png"),
      fullPage: true,
    });
    await page.context().route("https://auth.openai.com/**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "text/plain",
        body: "Synthetic test: external sign-in not performed.",
      }),
    );
    await page.goto(front + "/settings/connections?add=1");
    await page.getByText("OpenAI Codex", { exact: true }).click();
    assert.equal(
      await page
        .getByRole("radio", { name: "OpenAI Codex", exact: true })
        .isChecked(),
      true,
    );
    await page.getByRole("button", { name: "Continue", exact: true }).click();
    await page
      .getByRole("button", { name: "Sign in with Codex", exact: true })
      .waitFor();
    await page
      .getByRole("button", { name: "Sign in with Codex", exact: true })
      .click();
    await page
      .getByText("Waiting for browser authorization…", { exact: true })
      .waitFor();
    assert.equal(
      (await request("/api/settings/providers/openai-codex/oauth/status"))
        .operation_state,
      "waiting",
    );
    await page.screenshot({
      path: path.join(artifacts, "codex-waiting.png"),
      fullPage: true,
    });
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page
      .getByText("Codex sign-in was cancelled.", { exact: true })
      .waitFor();
    assert.equal(
      (await request("/api/settings/providers/openai-codex/oauth/status"))
        .operation_state,
      "cancelled",
    );
    assert.deepEqual(errors, []);
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
  assert.equal(
    (await request(`/api/study/${quiz.id}?workspace_id=${scope}`)).phase,
    "published",
  );
  assert.equal(
    (await request("/api/study/progress?workspace_id=" + scope)).total,
    process.env.WEB_QA === "1" ? 2 : 1,
  );
  const registration = (
    await request("/api/settings/workspace/registrations")
  ).workspaces.find((w) => w.workspace_id === scope);
  // Rust canonicalization retains the Windows \\?\ prefix; Node realpath
  // may omit it. Compare actual filesystem paths and path components.
  const restoredWorkspace = await realpath(registration.path);
  const relativeWorkspace = path.relative(await realpath(restored), restoredWorkspace);
  assert.ok(relativeWorkspace && !path.isAbsolute(relativeWorkspace));
  assert.ok(relativeWorkspace !== '..' && !relativeWorkspace.startsWith('..' + path.sep));
  assert.notEqual(restoredWorkspace, await realpath(workspace.path));
  const result = {
    passed: true,
    provider: "local synthetic Chat Completions SSE",
    calls,
    checks: [
      "workspace revision conflicts and archive admission",
      "workspace instructions in model requests",
      "source-required research admission",
      "committed research plan and verified evidence",
      "structured practice generation and answer receipts",
      "acyclic learning path and bound objective",
      "workspace isolation",
      "submission deduplication",
      "saved notes",
      "reviewed AI note edits",
      "memory consolidation proposals",
      "backup restore",
      "native provenance graph and workspace isolation",
      "automatic reviewed cleanup and revision fences",
      "retry-safe DOCX text import",
      ...(process.env.OCR_QA === "1" && importPolicy.ocr_available
        ? ["real native bilingual image and scanned PDF OCR"]
        : []),
      ...(process.env.WEB_QA === "1"
        ? [
            "production workspace and study UI",
            "practice reload",
            "mobile layout",
            "Codex login start and cancellation UI",
          ]
        : []),
    ],
    limits: ["no real provider or account verification"],
  };
  await writeFile(
    path.join(
      artifacts,
      process.env.WEB_QA === "1" ? "result.json" : "result-http.json",
    ),
    JSON.stringify(result, null, 2),
  );
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  console.error(logs.slice(-6000));
  throw error;
} finally {
  if (browser) await browser.close();
  await stop(frontend);
  await stop(backend);
  model.closeAllConnections();
  await new Promise((r) => model.close(r));
  await rm(temp, { recursive: true, force: true });
}
