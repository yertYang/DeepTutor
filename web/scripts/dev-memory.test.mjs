import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import memory from "./dev-memory.cjs";

test("budgets V8 and native cache independently on a 16 GB machine", () => {
  const settings = memory.devMemorySettings({}, 16 * 1024);
  assert.equal(settings.nodeOptions, "--max-old-space-size=2048");
  assert.equal(settings.turboBytes, 1024 ** 3);
});

test("scales both budgets down for constrained containers", () => {
  const settings = memory.devMemorySettings({}, 2048);
  assert.equal(settings.nodeOptions, "--max-old-space-size=512");
  assert.equal(settings.turboBytes, 256 * 1024 ** 2);
});

test("preserves explicit V8 ceilings and other inherited options", () => {
  for (const option of [
    "--max-old-space-size=3072",
    "--max_old_space_size=3072",
    "--max-old-space-size 3072",
  ]) {
    const inherited = `--enable-source-maps ${option}`;
    assert.equal(
      memory.devMemorySettings({ NODE_OPTIONS: inherited }).nodeOptions,
      inherited,
    );
  }
  assert.equal(
    memory.devMemorySettings({ NODE_OPTIONS: "--enable-source-maps" }, 16 * 1024).nodeOptions,
    "--enable-source-maps --max-old-space-size=2048",
  );
});

test("allows independent overrides and rejects malformed budgets", () => {
  const settings = memory.devMemorySettings({
    DEEPTUTOR_DEV_HEAP_MB: "3072",
    DEEPTUTOR_TURBO_MEMORY_MB: "768",
  });
  assert.equal(settings.nodeOptions, "--max-old-space-size=3072");
  assert.equal(settings.turboBytes, 768 * 1024 ** 2);
  for (const name of ["DEEPTUTOR_DEV_HEAP_MB", "DEEPTUTOR_TURBO_MEMORY_MB"]) {
    for (const value of ["0", "-1", "NaN", "1.5"]) {
      assert.throws(() => memory.devMemorySettings({ [name]: value }), /positive integer/);
    }
  }
});

test("a child Node process actually receives the heap ceiling", () => {
  const { nodeOptions } = memory.devMemorySettings({
    DEEPTUTOR_DEV_HEAP_MB: "512",
    NODE_OPTIONS: "--max-semi-space-size=8",
  });
  const child = spawnSync(process.execPath, [
    "-p", "require('node:v8').getHeapStatistics().heap_size_limit / 1024 / 1024",
  ], { env: { ...process.env, NODE_OPTIONS: nodeOptions }, encoding: "utf8" });
  assert.equal(child.status, 0, child.stderr);
  // V8 reserves a small young generation in addition to the old-space budget.
  assert.ok(Number(child.stdout) >= 512 && Number(child.stdout) < 600, child.stdout);
});
