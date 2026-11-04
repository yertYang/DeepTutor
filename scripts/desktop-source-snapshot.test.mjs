import assert from 'node:assert/strict';
import { test } from 'node:test';
import { includeSource } from '../apps/desktop/scripts/lib/source-snapshot.mjs';

test('desktop snapshots include native source folders and exclude generated artifacts and secrets', () => {
  for (const path of ['web/components/reading/native/NativeReadingLibrary.tsx', 'rust/server/src/speech.rs', 'packages/browser-testing/session.mjs']) {
    assert.equal(includeSource(path), true, path);
  }
  for (const path of ['native/deeptutor-server', 'web/.env.local', 'web/.next/server.js', 'web/node_modules/next/server.js', 'data/user/settings/auth.json', 'target/release/deeptutor-server']) {
    assert.equal(includeSource(path), false, path);
  }
});
