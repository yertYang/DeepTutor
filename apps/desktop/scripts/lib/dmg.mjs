import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, rename, rm } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
const execute = promisify(execFile);

/** Publish only a complete verified image; a failed build keeps the last one. */
export async function createDiskImage(source, destination, {
  run = execute,
  wait = delay,
  attempts = 4,
  log = console.warn,
} = {}) {
  if (!Number.isInteger(attempts) || attempts < 1 || attempts > 5) {
    throw new Error('DMG attempts must be between one and five.');
  }
  await mkdir(path.dirname(destination), { recursive: true });
  const folder = await mkdtemp(path.join(path.dirname(destination), '.lorevia-dmg-'));
  const temporary = path.join(folder, 'candidate.dmg');
  try {
    for (let attempt = 1; attempt <= attempts; attempt++) {
      await rm(temporary, { force: true });
      try {
        await run('/usr/bin/hdiutil', [
          'create', '-volname', 'Lorevia', '-srcfolder', source,
          '-fs', 'APFS', '-format', 'ULFO', temporary,
        ], { encoding: 'utf8', maxBuffer: 1024 * 1024 });
        await run('/usr/bin/hdiutil', ['verify', temporary], { encoding: 'utf8', maxBuffer: 1024 * 1024 });
        await rename(temporary, destination);
        return destination;
      } catch (error) {
        const busy = error?.code === 'EBUSY' || /hdiutil:[^\n]*(?:resource busy|device busy)/i.test(String(error?.stderr || ''));
        // Retry the observed macOS disk-image contention only. Invalid sources,
        // verification failures and permission errors must remain visible.
        if (attempt === attempts || !busy) throw error;
        const milliseconds = 2000 * 2 ** (attempt - 1);
        log(`DMG system resource is busy; retrying in ${milliseconds / 1000}s (${attempt}/${attempts}).`);
        await wait(milliseconds);
      }
    }
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
}
