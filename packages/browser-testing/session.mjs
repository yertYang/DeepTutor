// A conversation owns this process and its isolated browser context. Requests
// are serialized; page content is data and is never executed as agent code.
import {chromium} from 'playwright';
import {expect} from 'playwright/test';
import {existsSync} from 'node:fs';
import readline from 'node:readline';
import {allowedUrl, locator, validateSteps} from './scenario.mjs';
import {testProxy} from './proxy.mjs';

let browser, context, page, proxy, config, idle, closing = false;
const errors = [];
async function close() {
  if (closing) return;
  closing = true;
  clearTimeout(idle);
  const deadline = setTimeout(() => process.exit(0), 3000);
  await browser?.close().catch(() => {});
  await proxy?.close().catch(() => {});
  clearTimeout(deadline);
  process.exit(0);
}
process.stdin.on('end', close);
process.on('SIGTERM', close);
process.on('SIGINT', close);
function touch() { clearTimeout(idle); idle = setTimeout(close, 10 * 60 * 1000); }
touch();

async function run(request) {
  if (request.v !== 1 || !['open', 'snapshot', 'act', 'close'].includes(request.operation)) throw new Error('Invalid interactive browser request.');
  if (request.operation === 'close') return {ok: true, closed: true};
  const next = request.config;
  if (!next || !Array.isArray(next.allowed_origins) || !next.allowed_origins.length || next.allowed_origins.length > 64) throw new Error('Provide the website URL in your message first.');
  // The proxy reads this object on every connection, including redirects.
  if (config) Object.assign(config, next);
  else config = {...next};
  const steps = request.operation === 'open' ? [{action: 'navigate', url: request.url ?? config.base_url}] : request.operation === 'act' ? validateSteps(request.steps, {requireNavigation: false}) : [];
  for (const step of steps) if (['navigate', 'expect_url'].includes(step.action)) allowedUrl(step.url ?? step.value, config.base_url, config.allowed_origins);
  if (!browser?.isConnected() || !page) {
    await browser?.close().catch(() => {});
    await proxy?.close();
    page = undefined;
    proxy = await testProxy(config);
    const options = {headless: true, timeout: 20000, proxy: {server: proxy.url, bypass: '<-loopback>'}, args: ['--force-webrtc-ip-handling-policy=disable_non_proxied_udp', '--dns-prefetch-disable']};
    if (process.env.DEEPTUTOR_BROWSER_EXECUTABLE) options.executablePath = process.env.DEEPTUTOR_BROWSER_EXECUTABLE;
    else if (existsSync(chromium.executablePath())) options.executablePath = chromium.executablePath();
    else options.channel = 'chrome';
    browser = await chromium.launch(options);
    context = await browser.newContext({viewport: {width: 1280, height: 800}, acceptDownloads: false, serviceWorkers: 'block'});
    const observe = p => {
      p.setDefaultTimeout(5000); p.setDefaultNavigationTimeout(15000);
      p.on('dialog', d => d.dismiss());
      p.on('pageerror', e => { if (errors.length < 20) errors.push(e.message.slice(0, 500)); });
      p.on('popup', popup => { page = popup; });
    };
    context.on('page', observe);
    page = await context.newPage();
    // Login windows and target=_blank links remain in this isolated context.
  }
  if (page.isClosed()) page = context.pages().find(p => !p.isClosed()) ?? await context.newPage();
  const began = Date.now(), results = [];
  for (const [index, step] of steps.entries()) {
    const start = Date.now();
    try {
      const target = step.target ? locator(page, step.target) : null;
      switch (step.action) {
        case 'navigate': {
          const response = await page.goto(allowedUrl(step.url, config.base_url, config.allowed_origins), {waitUntil: 'domcontentloaded'});
          if (response?.status() >= 400) throw new Error(`Navigation returned HTTP ${response.status()}.`);
          break;
        }
        case 'click': await target.click(); break;
        case 'fill': await target.fill(step.value); break;
        case 'press': await target.press(step.value); break;
        case 'select': await target.selectOption(step.value); break;
        case 'check': await target.setChecked(step.checked); break;
        case 'expect_visible': await expect(target).toBeVisible(); break;
        case 'expect_text': await expect(target).toContainText(step.value); break;
        case 'expect_url': await expect(page).toHaveURL(allowedUrl(step.value, config.base_url, config.allowed_origins)); break;
        case 'expect_title': await expect(page).toHaveTitle(step.value); break;
        case 'expect_count': await expect(target).toHaveCount(step.count); break;
        default: throw new Error('Unsupported browser action.');
      }
      results.push({index, action: step.action, status: 'passed', duration_ms: Date.now() - start});
    } catch (error) {
      results.push({index, action: step.action, status: 'failed', duration_ms: Date.now() - start, error: error.message.slice(0, 3000)});
      results.push(...steps.slice(index + 1).map((s, i) => ({index: index + i + 1, action: s.action, status: 'skipped', duration_ms: 0})));
      break;
    }
  }
  const snapshot = await page.evaluate(() => ({
    text: document.body?.innerText.slice(0, 12000) ?? '',
    elements: Array.from(document.querySelectorAll('a,button,input,select,textarea,[role]')).filter(e => e.getClientRects().length).slice(0, 100).map(e => ({
      tag: e.tagName.toLowerCase(), id: e.id || null, role: e.getAttribute('role'), type: e.getAttribute('type'),
      name: (e.getAttribute('aria-label') || e.labels?.[0]?.innerText || e.innerText || '').slice(0, 160),
      test_id: e.getAttribute('data-testid'), placeholder: e.getAttribute('placeholder'),
    })),
  })).catch(() => ({text: '', elements: []}));
  const shot = await page.screenshot({type: 'png', timeout: 5000}).catch(() => null);
  const status = results.some(r => r.status === 'failed') ? 'failed' : steps.some(s => s.action.startsWith('expect_')) ? 'passed' : 'inspected';
  return {ok: true, artifacts: shot && shot.length <= 2 * 1024 * 1024 ? [{name: 'screenshot.png', mime: 'image/png', data: shot.toString('base64')}] : [], report: {
    status, steps: results, duration_ms: Date.now() - began, url: page.url(), title: await page.title().catch(() => ''), snapshot,
    console_errors: errors.splice(0), blocked_origins: [...proxy.blockedOrigins], session_active: true,
  }};
}

const input = readline.createInterface({input: process.stdin, crlfDelay: Infinity});
for await (const line of input) {
  clearTimeout(idle);
  const deadline = setTimeout(close, 65000);
  try {
    if (Buffer.byteLength(line) > 256 * 1024) throw new Error('Browser request exceeds size limit.');
    const result = await run(JSON.parse(line));
    process.stdout.write(JSON.stringify(result) + '\n');
    if (result.closed) await close();
  } catch (error) {
    process.stdout.write(JSON.stringify({ok: false, error: error.message.slice(0, 3000)}) + '\n');
  } finally { clearTimeout(deadline); touch(); }
}
