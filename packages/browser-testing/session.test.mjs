import assert from 'node:assert/strict';
import {test} from 'node:test';
import {createServer} from 'node:http';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import readline from 'node:readline';

test('browser actions retain live state and login, isolate sessions, and report real failures', {timeout: 90000}, async () => {
  let writes = 0;
  const website = createServer((req, res) => {
    if (req.url === '/write') { writes++; res.end('written'); return; }
    if (req.url === '/login') { res.setHeader('Set-Cookie', 'logged_in=yes; Path=/'); res.end('Logged in'); return; }
    if (req.url === '/account') { res.end(req.headers.cookie?.includes('logged_in=yes') ? 'Private account' : 'Anonymous'); return; }
    if (req.url === '/redirect') { res.writeHead(302, {location: 'http://localhost:12345/blocked'}).end(); return; }
    res.setHeader('Content-Type', 'text/html');
    res.end(`<title>Test fixture</title><label>Name<input aria-label="Name"></label><button onclick="document.querySelector('#result').textContent=document.querySelector('input').value;fetch('/login')">Login</button><p id="result">Not logged in</p><a href="/account">Account</a><button onclick="fetch('/write')">Write</button>`);
  });
  website.listen(0, '127.0.0.1'); await once(website, 'listening');
  const base = `http://127.0.0.1:${website.address().port}`;
  const config = {base_url: base, allowed_origins: [base], blocked_ports: []};
  const children = [];
  function session() {
    const child = spawn(process.execPath, [new URL('./session.mjs', import.meta.url).pathname], {stdio: ['pipe', 'pipe', 'pipe']});
    children.push(child);
    let stderr = ''; child.stderr.on('data', chunk => { stderr += chunk; });
    const lines = readline.createInterface({input: child.stdout})[Symbol.asyncIterator]();
    return {child, async call(operation, input = {}) {
      child.stdin.write(JSON.stringify({v: 1, operation, config, ...input}) + '\n');
      const {value, done} = await lines.next();
      assert.ok(!done, `Worker exited: ${stderr}`);
      const result = JSON.parse(value);
      return result;
    }};
  }
  try {
    const first = session();
    let result = await first.call('open', {url: base});
    assert.equal(result.ok, true, result.error);
    assert.equal(result.report.status, 'inspected');
    assert.ok(result.artifacts.some(a => a.mime === 'image/png'));
    result = await first.call('act', {steps: [
      {action: 'fill', target: {by: 'label', value: 'Name'}, value: 'Alice'},
      {action: 'click', target: {by: 'role', value: 'button', name: 'Login'}},
      {action: 'expect_text', target: {by: 'css', value: '#result'}, value: 'Alice'},
    ]});
    assert.equal(result.report.status, 'passed', JSON.stringify(result));
    result = await first.call('snapshot');
    assert.match(result.report.snapshot.text, /Alice/);
    result = await first.call('act', {steps: [
      {action: 'click', target: {by: 'role', value: 'link', name: 'Account'}},
      {action: 'expect_text', target: {by: 'css', value: 'body'}, value: 'Private account'},
    ]});
    assert.equal(result.report.status, 'passed', JSON.stringify(result));
    const second = session();
    result = await second.call('open', {url: `${base}/account`});
    assert.match(result.report.snapshot.text, /Anonymous/);
    result = await first.call('open', {url: base});
    result = await first.call('act', {steps: [
      {action: 'expect_text', target: {by: 'css', value: '#result'}, value: 'impossible'},
      {action: 'click', target: {by: 'role', value: 'button', name: 'Write'}},
    ]});
    assert.equal(result.report.status, 'failed');
    assert.equal(result.report.steps[1].status, 'skipped');
    assert.equal(writes, 0);
    result = await first.call('act', {steps: [{action: 'expect_text', target: {by: 'css', value: '#result'}, value: 'Not logged in'}]});
    assert.equal(result.report.status, 'passed');
    result = await first.call('open', {url: 'http://localhost:1/blocked'});
    assert.equal(result.ok, false);
    assert.match(result.error, /outside/);
    result = await first.call('open', {url: `${base}/redirect`});
    assert.equal(result.report.status, 'failed');
    assert.ok(result.report.blocked_origins.includes('http://localhost:12345'));
    result = await first.call('close');
    assert.equal(result.closed, true);
    await once(first.child, 'exit');
    second.child.stdin.end();
    await once(second.child, 'exit');
  } finally {
    for (const child of children) {
      if (child.exitCode === null) { const done = once(child, 'exit'); child.stdin.end(); await done; }
    }
    await new Promise(resolve => website.close(resolve));
  }
});
