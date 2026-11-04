import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createDiskImage } from '../apps/desktop/scripts/lib/dmg.mjs';
const exec = promisify(execFile);
const temporary = async fn => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'lorevia-dmg-test-'));
  try { await fn(root); } finally { await rm(root, { recursive: true, force: true }); }
};
const busy = () => Object.assign(new Error('Command failed'), { stderr: 'hdiutil: create failed - Resource busy' });

test('busy creation cannot publish partial images and a verified retry replaces the old image', () => temporary(async root => {
  const output = path.join(root, 'Lorevia.dmg');
  await writeFile(output, 'previous verified image');
  let calls = 0, delays = 0;
  await createDiskImage('synthetic source', output, {
    wait: async () => { delays++; }, log: () => {},
    run: async (_command, args) => {
      calls++;
      if (args[0] === 'create') {
        assert.equal(await readFile(output, 'utf8'), 'previous verified image');
        await writeFile(args.at(-1), calls === 1 ? 'partial' : 'new verified image');
        if (calls === 1) throw busy();
      } else {
        assert.equal(args[0], 'verify');
        assert.equal(await readFile(args[1], 'utf8'), 'new verified image');
      }
    },
  });
  assert.equal(delays, 1);
  assert.equal(await readFile(output, 'utf8'), 'new verified image');
  assert.deepEqual(await readdir(root), ['Lorevia.dmg']);
}));

test('a non-transient verification failure preserves the existing published image', () => temporary(async root => {
  const output = path.join(root, 'Lorevia.dmg');
  await writeFile(output, 'previous verified image');
  let calls = 0;
  await assert.rejects(createDiskImage('source', output, {
    wait: async () => assert.fail('Verification failure must not be retried'), log: () => {},
    run: async (_command, args) => {
      calls++;
      if (args[0] === 'create') await writeFile(args.at(-1), 'broken image');
      else throw Object.assign(new Error('Verification failed'), { stderr: 'hdiutil: checksum invalid' });
    },
  }), /Verification failed/);
  assert.equal(calls, 2);
  assert.equal(await readFile(output, 'utf8'), 'previous verified image');
  assert.deepEqual(await readdir(root), ['Lorevia.dmg']);
}));

test('persistent contention is bounded and leaves no partial published asset', () => temporary(async root => {
  const output = path.join(root, 'Lorevia.dmg');
  let calls = 0;
  await assert.rejects(createDiskImage('source', output, {
    wait: async () => {}, log: () => {},
    run: async (_command, args) => { calls++; await writeFile(args.at(-1), 'partial'); throw busy(); },
  }), /Command failed/);
  assert.equal(calls, 4);
  assert.deepEqual(await readdir(root), []);
}));

test('a source path containing Resource busy does not cause non-transient errors to retry', () => temporary(async root => {
  let calls = 0;
  await assert.rejects(createDiskImage('/source/Resource busy', path.join(root, 'Lorevia.dmg'), {
    wait: async () => assert.fail('Invalid source must not be retried'), log: () => {},
    run: async () => {
      calls++;
      throw Object.assign(new Error('Invalid source'), {
        stderr: 'source /source/Resource busy\nhdiutil: create failed - No such file or directory',
      });
    },
  }), /Invalid source/);
  assert.equal(calls, 1);
  assert.deepEqual(await readdir(root), []);
}));

test('macOS builds, verifies and mounts a real small image', { skip: process.platform !== 'darwin', timeout: 60000 }, () => temporary(async root => {
  const source = path.join(root, 'source');
  const mount = path.join(root, 'mounted');
  const output = path.join(root, 'Lorevia.dmg');
  await mkdir(source); await mkdir(mount);
  await writeFile(path.join(source, 'synthetic-marker.txt'), 'DMG packaging acceptance');
  await symlink('/Applications', path.join(source, 'Applications'));
  await createDiskImage(source, output);
  let attached = false;
  try {
    await exec('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, output]);
    attached = true;
    assert.equal(await readFile(path.join(mount, 'synthetic-marker.txt'), 'utf8'), 'DMG packaging acceptance');
  } finally {
    if (attached) await exec('/usr/bin/hdiutil', ['detach', mount]);
  }
}));
