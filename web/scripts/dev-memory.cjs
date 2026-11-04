const { readFileSync } = require("node:fs");
const os = require("node:os");

// Turbopack's Rust cache lives outside V8's heap. Budget both independently.
function memoryBudgetMB() {
  let bytes = os.totalmem();
  for (const file of [
    "/sys/fs/cgroup/memory.max",
    "/sys/fs/cgroup/memory/memory.limit_in_bytes",
  ]) {
    try {
      const limit = Number(readFileSync(file, "utf8").trim());
      if (Number.isFinite(limit) && limit > 0) bytes = Math.min(bytes, limit);
    } catch {
      // No cgroup limit on macOS/Windows or an unconstrained container.
    }
  }
  return Math.floor(bytes / 1024 / 1024);
}

function positiveMB(env, name, fallback) {
  if (!env[name]) return fallback;
  const value = Number(env[name]);
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive integer in MB`);
  }
  return value;
}

function devMemorySettings(env = process.env, budgetMB = memoryBudgetMB()) {
  const heapMB = positiveMB(
    env,
    "DEEPTUTOR_DEV_HEAP_MB",
    Math.max(128, Math.min(2048, Math.floor(budgetMB / 4))),
  );
  const turboMB = positiveMB(
    env,
    "DEEPTUTOR_TURBO_MEMORY_MB",
    Math.max(128, Math.min(1024, Math.floor(budgetMB / 8))),
  );
  const inherited = env.NODE_OPTIONS ?? "";
  return {
    nodeOptions: /--max[-_]old[-_]space[-_]size(?:[=\s]|$)/.test(inherited)
      ? inherited
      : `${inherited} --max-old-space-size=${heapMB}`.trim(),
    turboBytes: turboMB * 1024 * 1024,
  };
}

module.exports = { devMemorySettings };
