import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { mkdir, rename } from 'node:fs/promises';
import path from 'node:path';

const exec = promisify(execFile);
// Call after stopping the supervisor. The native command acquires the home
// lock and snapshots both its data and registered external workspaces.
export async function createUpgradeBackup({ serverPath, runtimeHome, backupRoot }) {
  await mkdir(backupRoot, { recursive: true, mode: 0o700 });
  const backup = path.join(backupRoot, `before-upgrade-${randomUUID()}`);
  await exec(serverPath, ['--home', runtimeHome, 'backup', '--output', backup], { timeout: 600_000, maxBuffer: 1024 * 1024 });
  return backup;
}

export async function restoreUpgradeBackup({ serverPath, runtimeHome, backup }) {
  // Verify with the previous executable before moving any current data. Keep
  // interrupted homes intact so a failed restore can be retried on next boot.
  const verified = await exec(serverPath, ['verify-backup', '--from', backup], { timeout: 600_000, maxBuffer: 1024 * 1024 });
  if (JSON.parse(verified.stdout).compatible !== true) throw new Error('The previous runtime cannot restore this backup schema.');
  const preserved = `${runtimeHome}.interrupted-${randomUUID()}`;
  try { await rename(runtimeHome, preserved); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  // Native workspace paths are rebased to this home. Restoring elsewhere then
  // renaming would leave stale paths, so restore directly at the final path.
  await exec(serverPath, ['--home', runtimeHome, 'restore', '--from', backup], { timeout: 600_000, maxBuffer: 1024 * 1024 });
  return preserved;
}
