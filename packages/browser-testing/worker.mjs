import {chromium} from 'playwright';
import {expect} from 'playwright/test';
import {mkdtemp, readFile, rm, stat} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import {allowedUrl, generateSpec, locator, validateSteps} from './scenario.mjs';
import {testProxy} from './proxy.mjs';

let browser, activeProxy, temporaryRoot, aborted = false;
async function abort() {
  if (aborted) return;
  aborted = true;
  const deadline = setTimeout(() => process.exit(1), 3000);
  try { await browser?.close(); await activeProxy?.close(); if (temporaryRoot) await rm(temporaryRoot, {recursive: true, force: true}); }
  finally { clearTimeout(deadline); process.exit(1); }
}
process.stdin.on('end', abort);
process.on('SIGTERM', abort);
process.on('SIGINT', abort);

async function run(request) {
  if (request.v !== 1 || !['inspect', 'run', 'export', 'status'].includes(request.operation)) throw new Error('Invalid browser worker protocol.');
  if (request.operation === 'status') return {ok: true, bundled: existsSync(chromium.executablePath()), engine: 'chromium'};
  const {base_url: baseUrl, allowed_origins: origins} = request.config;
  if (!Array.isArray(origins) || !origins.length || origins.length > 16) throw new Error('Configure allowed websites first.');
  const steps = validateSteps(request.steps);
  if (request.operation === 'run' && !steps.some(s => s.action.startsWith('expect_'))) throw new Error('A test must include at least one expectation.');
  for (const s of steps) if (s.action === 'navigate' || s.action === 'expect_url') allowedUrl(s.url ?? s.value, baseUrl, origins);
  const source = generateSpec(request.title || 'Browser test', steps, baseUrl, origins);
  if (request.operation === 'export') return {ok: true, source};
  const temporary = await mkdtemp(path.join(tmpdir(), 'yanzhizhou-browser-'));
  temporaryRoot = temporary;
  const artifacts = [], consoleErrors = [], results = [];
  const began = Date.now();
  let timer, proxy;
  try {
    proxy = await testProxy(request.config);
    activeProxy = proxy;
    const options = {headless: true, timeout: 20000, proxy: {server: proxy.url, bypass: '<-loopback>'}, args: ['--force-webrtc-ip-handling-policy=disable_non_proxied_udp', '--dns-prefetch-disable']};
    if (process.env.DEEPTUTOR_BROWSER_EXECUTABLE) options.executablePath = process.env.DEEPTUTOR_BROWSER_EXECUTABLE;
    else if (existsSync(chromium.executablePath())) options.executablePath = chromium.executablePath();
    else options.channel = 'chrome';
    browser = await chromium.launch(options);
    if (aborted) throw new Error('Browser run cancelled.');
    timer = setTimeout(abort, 65000);
    const context = await browser.newContext({viewport: {width: 1280, height: 800}, acceptDownloads: false, serviceWorkers: 'block'});
    await context.tracing.start({screenshots: true, snapshots: true});
    const page = await context.newPage();
    page.setDefaultTimeout(5000); page.setDefaultNavigationTimeout(15000);
    page.on('dialog', d => d.dismiss());
    page.on('popup', p => p.close());
    page.on('pageerror', e => { if (consoleErrors.length < 20) consoleErrors.push(e.message.slice(0, 500)); });
    for (const [index, step] of steps.entries()) {
      const start = Date.now();
      try {
        const target = step.target ? locator(page, step.target) : null;
        switch (step.action) {
          case 'navigate': {
            const response = await page.goto(allowedUrl(step.url, baseUrl, origins), {waitUntil: 'domcontentloaded'});
            if (response && response.status() >= 400) throw new Error(`Navigation returned HTTP ${response.status()}.`);
            break;
          }
          case 'click': await target.click(); break;
          case 'fill': await target.fill(step.value); break;
          case 'press': await target.press(step.value); break;
          case 'select': await target.selectOption(step.value); break;
          case 'check': await target.setChecked(step.checked); break;
          case 'expect_visible': await expect(target).toBeVisible(); break;
          case 'expect_text': await expect(target).toContainText(step.value); break;
          case 'expect_url': await expect(page).toHaveURL(allowedUrl(step.value, baseUrl, origins)); break;
          case 'expect_title': await expect(page).toHaveTitle(step.value); break;
          case 'expect_count': await expect(target).toHaveCount(step.count); break;
          default: throw new Error('Unsupported step.');
        }
        results.push({index, action: step.action, status: 'passed', duration_ms: Date.now() - start});
      } catch (error) {
        results.push({index, action: step.action, status: 'failed', duration_ms: Date.now() - start, error: error.message.slice(0, 3000)});
        results.push(...steps.slice(index + 1).map((s, i) => ({index: index + i + 1, action: s.action, status: 'skipped', duration_ms: 0})));
        break;
      }
    }
    const status = results.some(s => s.status === 'failed') ? 'failed' : request.operation === 'inspect' ? 'inspected' : 'passed';
    // Snapshot extraction is fixed application code, never model-supplied code.
    const snapshot = await page.evaluate(() => ({
      text: document.body?.innerText.slice(0, 12000) ?? '',
      elements: Array.from(document.querySelectorAll('a,button,input,select,textarea,[role]')).filter(e => e.getClientRects().length).slice(0, 80).map(e => ({
        tag: e.tagName.toLowerCase(), role: e.getAttribute('role'), type: e.getAttribute('type'),
        name: (e.getAttribute('aria-label') || e.labels?.[0]?.innerText || e.innerText || '').slice(0, 160),
        test_id: e.getAttribute('data-testid'), placeholder: e.getAttribute('placeholder'),
      })),
    })).catch(() => ({text: '', elements: []}));
    const shot = await page.screenshot({type: 'png', timeout: 5000}).catch(() => null);
    if (shot && shot.length <= 2 * 1024 * 1024) artifacts.push({name: 'screenshot.png', mime: 'image/png', data: shot.toString('base64')});
    const trace = path.join(temporary, 'trace.zip');
    await context.tracing.stop(status === 'failed' ? {path: trace} : {});
    if (status === 'failed' && (await stat(trace)).size <= 8 * 1024 * 1024) artifacts.push({name: 'trace.zip', mime: 'application/zip', data: (await readFile(trace)).toString('base64')});
    return {ok: true, source, artifacts, report: {status, steps: results, duration_ms: Date.now() - began, url: page.url(), title: await page.title().catch(() => ''), snapshot, console_errors: consoleErrors}};
  } finally {
    clearTimeout(timer);
    await browser?.close();
    browser = undefined;
    await proxy?.close();
    activeProxy = undefined;
    await rm(temporary, {recursive: true, force: true});
    temporaryRoot = undefined;
  }
}

const input = readline.createInterface({input: process.stdin, crlfDelay: Infinity});
input.once('line', async line => {
  try {
    if (Buffer.byteLength(line) > 256 * 1024) throw new Error('Browser request exceeds size limit.');
    const result = await run(JSON.parse(line));
    if (aborted) return;
    process.stdout.write(JSON.stringify(result) + '\n', () => process.exit(0));
  } catch (error) {
    if (aborted) return;
    process.stdout.write(JSON.stringify({ok: false, error: error.message.slice(0, 3000)}) + '\n', () => process.exit(0));
  }
});
