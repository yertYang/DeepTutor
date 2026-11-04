import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { mediaPermissionPolicy } from '../apps/desktop/src/media-permissions.mjs';

test('media grants require the active renderer, exact origin and separately approved media type', async () => {
  const contents = Object.assign(new EventEmitter(), { id: 1 });
  const other = Object.assign(new EventEmitter(), { id: 2 });
  let active = contents, url = 'http://127.0.0.1:4567', prompts = 0;
  const policy = mediaPermissionPolicy({ currentContents: () => active, currentUrl: () => url, ask: async () => { prompts++; return true; } });
  const request = async (who, origin, types) => {
    let result; await policy.request(who, 'media', value => { result = value; }, { requestingUrl: origin + '/voice', mediaTypes: types }); return result;
  };
  assert.equal(await request(other, url, ['audio']), false);
  assert.equal(await request(contents, 'http://127.0.0.1:4568', ['audio']), false);
  assert.equal(await request(contents, 'https://untrusted.example', ['audio']), false);
  assert.equal(prompts, 0);
  assert.equal(await request(contents, url, ['audio']), true);
  assert.equal(policy.check(contents, 'media', url, { mediaType: 'audio' }), true);
  assert.equal(policy.check(contents, 'media', url, { mediaType: 'video' }), false);
  assert.equal(await request(contents, url, ['audio']), true); assert.equal(prompts, 1);
  assert.equal(await request(contents, url, ['video']), true); assert.equal(prompts, 2);
  contents.emit('destroyed');
  assert.equal(policy.check(contents, 'media', url, { mediaType: 'audio' }), false);
  active = other;
  assert.equal(policy.check(contents, 'media', url, { mediaType: 'video' }), false);
});

test('denied and obsolete permission dialogs cannot publish a grant', async () => {
  const contents = Object.assign(new EventEmitter(), { id: 1 }); let active = contents;
  let resolve; const pending = new Promise(done => { resolve = done; });
  const policy = mediaPermissionPolicy({ currentContents: () => active, currentUrl: () => 'http://127.0.0.1:4567', ask: () => pending });
  let result;
  const request = policy.request(contents, 'media', value => { result = value; }, { requestingUrl: 'http://127.0.0.1:4567/voice', mediaTypes: ['audio'] });
  active = null; resolve(true); await request; assert.equal(result, false);
  active = contents;
  assert.equal(policy.check(contents, 'media', 'http://127.0.0.1:4567', { mediaType: 'audio' }), false);
  const denied = mediaPermissionPolicy({ currentContents: () => contents, currentUrl: () => 'http://127.0.0.1:4567', ask: async () => false });
  await denied.request(contents, 'media', value => { result = value; }, { requestingUrl: 'http://127.0.0.1:4567/voice', mediaTypes: ['audio'] });
  assert.equal(result, false);
  assert.equal(denied.check(contents, 'media', 'http://127.0.0.1:4567', { mediaType: 'audio' }), false);
});
