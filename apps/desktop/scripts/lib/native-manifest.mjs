import { readFile, writeFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { sha256 } from '../../src/bundles.mjs';

/** codesign changes executable bytes: checksums must describe the signed files. */
export async function refreshNativeManifest(runtime) {
  const directory = path.join(runtime, 'core/native');
  const file = path.join(directory, 'manifest.json');
  const manifest = JSON.parse(await readFile(file, 'utf8'));
  const suffix = manifest.target.includes('windows') ? '.exe' : '';
  const names = ['yanzhizhou', 'deeptutor', 'deeptutor-server'].map(name => name + suffix);
  if (manifest.schema_version !== 1 || JSON.stringify(Object.keys(manifest.files).sort()) !== JSON.stringify(names.sort())) {
    throw new Error('Invalid native runtime manifest');
  }
  for (const name of names) {
    const executable = path.join(directory, name);
    manifest.files[name] = { sha256: await sha256(executable), size: (await stat(executable)).size };
  }
  await writeFile(file, JSON.stringify(manifest, null, 2) + '\n');
  const infoFile = path.join(runtime, 'BUILD_INFO.json');
  const info = JSON.parse(await readFile(infoFile, 'utf8'));
  info.native = manifest;
  await writeFile(infoFile, JSON.stringify(info, null, 2) + '\n');
}
