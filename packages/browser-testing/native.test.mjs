// Build the native server first: cargo build -p deeptutor-server.
// The model is synthetic; browser actions, tool dispatch and reports are real.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {createServer} from 'node:http';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {mkdtemp, mkdir, writeFile, readFile, rm} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const repo = fileURLToPath(new URL('../../', import.meta.url));
const binary = process.env.NATIVE_TEST_BINARY ?? path.join(repo, 'target/debug/deeptutor-server' + (process.platform === 'win32' ? '.exe' : ''));

test('imported Excel and requirements drive browser tests and export actual results across turns', {skip: !existsSync(binary), timeout: 90000}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'browser-native-test-'));
  let backend, logs = '', calls = 0, base, workbookId, requirementsId;
  const website = createServer((req, res) => {
    res.setHeader('Content-Type', 'text/html');
    res.end('<label>Name<input aria-label="Name"></label><button onclick="document.querySelector(\'#result\').textContent=document.querySelector(\'input\').value">Apply</button><p id="result">Empty</p>');
  });
  const model = createServer(async (req, res) => {
    try {
      let raw = ''; for await (const chunk of req) raw += chunk;
      const body = JSON.parse(raw);
      const tool = body.tools.find(t => t.function?.name === 'browser_test');
      assert.ok(tool, 'Browser tool must be available without enabling website settings');
      assert.ok(tool.function.parameters.properties.action.enum.includes('act'));
      const latestUser = body.messages.findLastIndex(m => m.role === 'user');
      const results = body.messages.slice(latestUser + 1).filter(m => m.role === 'tool').map(m => JSON.parse(m.content));
      for (const result of results) assert.equal(result.success, true, JSON.stringify(result));
      const continuing = body.messages[latestUser].content.includes('Continue');
      let action;
      if (continuing) action=results.length===0?{action:'snapshot'}:results.length===1?{action:'act',steps:[{action:'expect_text',target:{by:'css',value:'#result'},value:'Alice'}]}:null;
      else action=[
        {action:'sources'},
        {action:'read_source',source_id:workbookId},
        {action:'read_source',source_id:requirementsId},
        {action:'open',url:base},
        {action:'act',steps:[
          {action: 'fill', target: {by: 'label', value: 'Name'}, value: 'Alice'},
          {action: 'click', target: {by: 'role', value: 'button', name: 'Apply'}},
          {action: 'expect_text', target: {by: 'css', value: '#result'}, value: 'Alice'},
        ]},
        {action:'update_workbook',source_id:workbookId,sheet:'Cases',expected_revision:0,rows:[{row:2,run_id:results[4]?.content?.id},{row:3,reason:'Credentials were not supplied'}]},
        {action:'read_source',source_id:workbookId},
      ][results.length]??null;
      if (continuing && results.length) assert.match(results[0].content.report.snapshot.text, /Alice/);
      if (continuing && results.length>1) assert.equal(results.at(-1).content.report.status,'passed');
      if (!continuing && results.length>6) {
        assert.match(results[6].content.content,/G2\t通过/);
        assert.match(results[6].content.content,/G3\t未执行/);
        assert.match(results[6].content.content,/F2\t2/);
      }
      calls++;
      const delta = action ? {tool_calls: [{index: 0, id: `browser-${calls}`, type: 'function', function: {name: 'browser_test', arguments: JSON.stringify(action)}}]} : {content: 'Verified.'};
      res.writeHead(200, {'content-type': 'text/event-stream'});
      res.write(`data: ${JSON.stringify({choices: [{index: 0, delta, finish_reason: null}]})}\n\n`);
      res.write(`data: ${JSON.stringify({choices: [{index: 0, delta: {}, finish_reason: action ? 'tool_calls' : 'stop'}]})}\n\n`);
      res.end('data: [DONE]\n\n');
    } catch (error) { res.writeHead(500); res.end(error.stack); }
  });
  const listen = async server => { server.listen(0, '127.0.0.1'); await once(server, 'listening'); return `http://127.0.0.1:${server.address().port}`; };
  try {
    base = await listen(website);
    const modelUrl = await listen(model);
    const settings = path.join(home, 'data/user/settings');
    await mkdir(settings, {recursive: true});
    await writeFile(path.join(settings, 'model_catalog.json'), JSON.stringify({version: 1, connections: [], services: {llm: {
      active_profile_id: 'qa', active_model_id: 'm', profiles: [{id: 'qa', name: 'Synthetic provider', binding: 'custom', api_key: 'synthetic-only', base_url: `${modelUrl}/v1`, api_format: 'chat_completions', models: [{id: 'm', name: 'Synthetic', model: 'browser-test', context_window: 200000, max_tokens: 2000}]}],
    }}}));
    // Ask the OS for a free backend port, then release it before spawning.
    const reserve = createServer(); await listen(reserve);
    const port = reserve.address().port;
    await new Promise(resolve => reserve.close(resolve));
    const origin = `http://127.0.0.1:${port}`;
    backend = spawn(binary, ['--home', home, 'serve', '--port', String(port)], {stdio: ['ignore', 'pipe', 'pipe'], env: {...process.env, DEEPTUTOR_AUTH_ENABLED: 'false', DEEPTUTOR_BROWSER_WORKER: path.join(repo, 'packages/browser-testing/worker.mjs')}});
    backend.stdout.on('data', chunk => { logs += chunk; });
    backend.stderr.on('data', chunk => { logs += chunk; });
    const until = async fn => {
      const deadline = Date.now() + 40000;
      while (Date.now() < deadline) {
        const value = await fn(); if (value) return value;
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw new Error(`Native test timed out: ${logs}`);
    };
    await until(async () => { try { return (await fetch(`${origin}/health`)).ok; } catch { return false; } });
    const request = async (route, body) => {
      const response = await fetch(origin + route, {method: body ? 'POST' : 'GET', headers: body ? {'content-type': 'application/json'} : {}, body: body ? JSON.stringify(body) : undefined});
      const result = await response.json();
      assert.equal(response.status, 200, JSON.stringify(result));
      return result;
    };
    const before = await request('/api/browser-testing');
    assert.equal(before.config.enabled, false);
    const completed = async turn => until(async () => {
      const {events} = await request(`/api/turns/${turn.turn_id}/events`);
      const error = events.find(event => event.type === 'error');
      assert.ok(!error, JSON.stringify(error));
      const done = events.find(event => event.type === 'done');
      if (!done) return false;
      assert.equal(done.metadata.status, 'completed', JSON.stringify(done));
      return events;
    });
    const fixture=await readFile(path.join(repo,'rust/server/tests/fixtures/browser-cases.xlsx'));
    const form=new FormData();
    form.append('files',new Blob([fixture]),'cases.xlsx');
    form.append('files',new Blob([`Test website: ${base}\nRead the Excel steps and verify the name.`]),'requirements.txt');
    const importedResponse=await fetch(`${origin}/api/browser-testing/sources`,{method:'POST',body:form});
    assert.equal(importedResponse.status,200,await importedResponse.clone().text());
    const imported=await importedResponse.json();
    workbookId=imported.sources.find(s=>s.name==='cases.xlsx').id;
    requirementsId=imported.sources.find(s=>s.name==='requirements.txt').id;
    assert.equal((await fetch(`${origin}/api/browser-testing/sources/${workbookId}?workspace_id=other`)).status,404);
    const original=await fetch(`${origin}/api/browser-testing/sources/${workbookId}/original`);
    assert.deepEqual(Buffer.from(await original.arrayBuffer()),fixture);
    const largeBatch=new FormData();
    largeBatch.append('files',new Blob(['supporting text\n'.repeat(80000)]),'support-a.txt');
    largeBatch.append('files',new Blob(['supporting text\n'.repeat(80000)]),'support-b.txt');
    const largeImport=await fetch(`${origin}/api/browser-testing/sources`,{method:'POST',body:largeBatch});
    assert.equal(largeImport.status,200,await largeImport.text());
    const first = await request('/api/turns', {capability: 'test_cases', content: `Read imported files ${workbookId} and ${requirementsId}, test the website and update the Excel cases.`, tools: [], client_submission_id: 'browser-first'});
    await completed(first);
    const second = await request('/api/turns', {session_id: first.session_id, capability: 'test_cases', content: 'Continue and verify the name is still Alice', tools: [], client_submission_id: 'browser-second'});
    await completed(second);
    const after = await request('/api/browser-testing');
    assert.equal(after.config.enabled, false);
    assert.equal(after.runs.length, 4);
    assert.equal(after.runs.filter(run => run.status === 'passed').length, 2);
    for (const run of after.runs) {
      assert.equal(run.report.session_active, true);
      assert.ok(run.artifacts.some(a => a.mime === 'image/png'));
    }
    assert.equal(after.exports.length,1);
    const exported=after.exports[0];
    const download=await fetch(origin+exported.download_url);
    assert.equal(download.status,200);
    assert.match(download.headers.get('content-type'),/spreadsheetml/);
    const updated=Buffer.from(await download.arrayBuffer());
    assert.notDeepEqual(updated,fixture);
    const unchanged=await fetch(`${origin}/api/browser-testing/sources/${workbookId}/original`);
    assert.deepEqual(Buffer.from(await unchanged.arrayBuffer()),fixture);
    assert.equal((await fetch(`${origin}/api/browser-testing/exports/${exported.id}?workspace_id=other`)).status,404);
    assert.equal(calls,11);
  } finally {
    if (backend && backend.exitCode === null) { const done = once(backend, 'exit'); backend.kill('SIGTERM'); await done; }
    await Promise.all([website, model].map(server => new Promise(resolve => server.close(resolve))));
    await rm(home, {recursive: true, force: true});
  }
});
