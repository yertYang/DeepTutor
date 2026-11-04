#!/usr/bin/env node
/**
 * Dev-server entrypoint that pins the V8 heap ceiling of the process that
 * actually holds the memory.
 *
 * `next dev` does not render in the process you launch — it spawns a separate
 * render worker, and V8 flags in argv are not inherited by child processes. So
 * the long-standing `node --max-old-space-size=4096 .../next dev` never reached
 * the worker: Next reads the ceiling from NODE_OPTIONS
 * (`getMaxOldSpaceSize()`), finds nothing, and substitutes 50% of total RAM
 * with no upper bound (`next dev` CLI, guarded by NEXT_DISABLE_MEM_OVERRIDE).
 * On a 24GB machine that is a 12GB heap, which V8 will genuinely try to fill:
 * an hour-long session died in `Ineffective mark-compacts near heap limit`
 * after spending 63 seconds inside a single mark-compact, with the machine
 * swapping long before that.
 *
 * Setting the ceiling here fixes both halves — the worker inherits NODE_OPTIONS,
 * and Next stops overriding it. Budget V8 separately from Turbopack's native
 * cache (see dev-memory.cjs and next.config.js). An explicit NODE_OPTIONS
 * ceiling from the developer always wins.
 */

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import memory from "./dev-memory.cjs";

const WEB_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const NEXT_BIN = path.join(WEB_DIR, "node_modules", "next", "dist", "bin", "next");

const { nodeOptions } = memory.devMemorySettings();
const args = process.argv.slice(2);
const lowMemory = args.includes("--low-memory");
const nextArgs = args.filter((arg) => arg !== "--low-memory");
if (lowMemory) {
  if (nextArgs.some((arg) => ["--turbo", "--turbopack"].includes(arg))) {
    throw new Error("--low-memory uses Webpack; remove --turbo/--turbopack");
  }
  if (!nextArgs.includes("--webpack")) nextArgs.push("--webpack");
}

const child = spawn(process.execPath, [NEXT_BIN, "dev", ...nextArgs], {
  cwd: WEB_DIR,
  stdio: "inherit",
  env: {
    ...process.env,
    NODE_OPTIONS: nodeOptions,
    ...(lowMemory ? { DEEPTUTOR_DEV_LOW_MEMORY: "1" } : {}),
  },
});

// Let Next own the terminal: forward the signals it already handles (lock file
// cleanup, worker teardown) instead of dying first and orphaning the worker.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => child.kill(signal));
}
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 0);
});
child.on("error", (error) => {
  console.error(error);
  process.exit(1);
});
