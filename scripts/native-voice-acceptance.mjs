// Native HTTP/WebSocket + actual browser Opus roundtrip using synthetic audio.
// Build Rust and WEB_QA_DIST (a standalone Next build) first; no GPU/model needed.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { once } from 'node:events'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
const root = fileURLToPath(new URL('../', import.meta.url))
const require = createRequire(path.join(root, 'web/package.json'))
const { WebSocketServer } = require('next/dist/compiled/ws')
const temp = await mkdtemp(path.join(os.tmpdir(), 'native-voice-qa-'))
const artifacts = path.join(root, 'dist/native-voice-acceptance')
await mkdir(artifacts, { recursive: true })
let backend, frontend, browser, logs = '', page
const until = async (fn, timeout = 30000) => {
  const end = Date.now() + timeout
  while (Date.now() < end) { if (await fn()) return; await new Promise(resolve => setTimeout(resolve, 100)) }
  throw new Error('Timed out: ' + logs.slice(-4000))
}
const port = async () => {
  const server = createServer().listen(0, '127.0.0.1'); await once(server, 'listening')
  const value = server.address().port; await new Promise(resolve => server.close(resolve)); return value
}
const stop = async child => {
  if (child && child.exitCode === null) { const done = once(child, 'exit'); child.kill('SIGTERM'); await done }
}
const upstream = createServer().listen(0, '127.0.0.1'); await once(upstream, 'listening')
const wsServer = new WebSocketServer({ server: upstream })
let packets = 0, ogg = false, upstreamCalls = 0, holdHandshake = false
wsServer.on('connection', (socket, request) => {
  upstreamCalls++
  assert.equal(request.headers.authorization, 'Bearer synthetic-voice-key')
  const url = new URL(request.url, 'http://localhost')
  assert.equal(url.searchParams.get('voice_prompt'), 'NATF2.pt')
  if (!holdHandshake) socket.send(Buffer.from([0]))
  let caption = false
  socket.on('message', (data, binary) => {
    assert.equal(binary, true); assert.equal(data[0], 1); packets++
    if (data.subarray(1, 5).toString() === 'OggS') ogg = true
    // Echo the real encoder's Opus pages into the real decoder/playback path.
    socket.send(data)
    if (!caption) { caption = true; socket.send(Buffer.from('\x02Hello, let us practice English.')) }
  })
})
try {
  const backendPort = await port(), webPort = await port()
  const origin = `http://127.0.0.1:${backendPort}`
  backend = spawn(path.resolve(process.env.NATIVE_TEST_BINARY || path.join(root, 'target/debug/deeptutor-server')),
    ['--home', path.join(temp, 'home'), 'serve', '--port', String(backendPort)], { stdio: ['ignore', 'pipe', 'pipe'] })
  for (const stream of [backend.stdout, backend.stderr]) stream.on('data', data => { logs += data })
  await until(async () => { try { return (await fetch(origin + '/health')).ok } catch { return false } })
  const register = async name => {
    const response = await fetch(origin + '/api/settings/workspace/registrations', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name }) })
    assert.equal(response.status, 200); return (await response.json()).workspace.workspace_id
  }
  const scopeA = await register('Voice QA A'), scopeB = await register('Voice QA B')
  const req = async (suffix, method = 'GET', body, scope = scopeA, expected = 200) => {
    const response = await fetch(`${origin}/api/voice${suffix}?dt_workspace=${scope}`, { method,
      ...(body === undefined ? {} : { headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }) })
    const value = await response.json(); assert.equal(response.status, expected, JSON.stringify(value)); return value
  }
  const input = { expected_revision: 0, enabled: true, endpoint: `ws://127.0.0.1:${upstream.address().port}/api/chat`,
    voice: 'NATF2.pt', prompt: 'Synthetic English coach', api_key: 'synthetic-voice-key' }
  const config = await req('/config', 'PUT', input)
  assert.equal(config.has_api_key, true); assert.ok(!JSON.stringify(config).includes(input.api_key))
  await req('/config', 'PUT', input, scopeA, 409)
  assert.equal((await req('', 'GET', undefined, scopeB)).config.enabled, false)
  console.log('PASS: configuration, redacted secrets, revision conflicts and workspace isolation')
  if (process.env.WEB_QA !== '1') process.exitCode = 0
  else {
    const dist = process.env.WEB_QA_DIST || '.next-voice-qa'
    const standalone = path.join(root, 'web', dist, 'standalone')
    frontend = spawn(process.execPath, [path.join(standalone, 'server.js')], { cwd: standalone,
      env: { ...process.env, PORT: String(webPort), HOSTNAME: '127.0.0.1', DEEPTUTOR_API_BASE_URL: origin, NEXT_TELEMETRY_DISABLED: '1' },
      stdio: ['ignore', 'pipe', 'pipe'] })
    for (const stream of [frontend.stdout, frontend.stderr]) stream.on('data', data => { logs += data })
    const web = `http://127.0.0.1:${webPort}`
    await until(async () => { try { return (await fetch(web + '/voice')).ok } catch { return false } })
    const wav = Buffer.alloc(44 + 48000 * 2 * 2)
    wav.write('RIFF'); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8); wav.writeUInt32LE(16, 16)
    wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22); wav.writeUInt32LE(48000, 24); wav.writeUInt32LE(96000, 28)
    wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34); wav.write('data', 36); wav.writeUInt32LE(wav.length - 44, 40)
    for (let i = 0; i < 96000; i++) wav.writeInt16LE(Math.round(5000 * Math.sin(i * Math.PI * 2 * 440 / 48000)), 44 + i * 2)
    const wavPath = path.join(temp, 'microphone.wav'); await writeFile(wavPath, wav)
    const { chromium } = await import('../packages/browser-testing/node_modules/playwright/index.mjs')
    browser = await chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || 'chrome', headless: true,
      args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', `--use-file-for-fake-audio-capture=${wavPath}`] })
    const context = await browser.newContext({ permissions: ['microphone'], viewport: { width: 1360, height: 950 } })
    await context.addInitScript(() => {
      window.__voiceTracks = []; window.__voiceContexts = []
      const original = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices)
      navigator.mediaDevices.getUserMedia = async options => {
        const stream = await original(options); window.__voiceTracks.push(...stream.getTracks()); return stream
      }
      const Base = window.AudioContext
      window.AudioContext = class extends Base { constructor(options) { super(options); window.__voiceContexts.push(this) } }
    })
    page = await context.newPage()
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    page.on('console', message => { if (message.type() === 'error') logs += '\nBROWSER: ' + message.text() })
    await page.goto(web + '/voice?dt_workspace=' + scopeA)
    const start = page.getByRole('button', { name: 'Start voice conversation', exact: true })
    await start.waitFor(); await until(async () => start.isEnabled())
    await start.click()
    await page.getByText('Voice conversation active', { exact: true }).waitFor({ timeout: 30000 })
    await page.getByText('Hello, let us practice English.', { exact: true }).waitFor()
    await until(async () => (await page.getByText(/Audio played:/).count()) > 0)
    assert.ok(packets > 3 && ogg); assert.equal(upstreamCalls, 1)
    await page.getByRole('button', { name: 'Mute microphone', exact: true }).click()
    assert.equal(await page.evaluate(() => window.__voiceTracks.every(track => !track.enabled)), true)
    await page.getByRole('button', { name: 'Unmute microphone', exact: true }).click()
    await page.getByRole('button', { name: 'Mute voice replies', exact: true }).click()
    await page.screenshot({ path: path.join(artifacts, 'voice-desktop.png'), fullPage: true })
    await page.getByRole('button', { name: 'End voice conversation', exact: true }).click()
    await until(async () => page.evaluate(() => window.__voiceTracks.every(track => track.readyState === 'ended') && window.__voiceContexts.every(context => context.state === 'closed')))
    assert.equal(wsServer.clients.size, 0)
    assert.deepEqual(errors, [])
    console.log('PASS: actual microphone Opus encoding, native WS proxy, WASM decoding and AudioWorklet playback; mute/end resource cleanup')
    holdHandshake = true
    await start.click()
    await until(async () => wsServer.clients.size > 0)
    await page.getByRole('button', { name: 'End voice conversation', exact: true }).click()
    await until(async () => wsServer.clients.size === 0)
    await until(async () => page.evaluate(() => window.__voiceTracks.every(track => track.readyState === 'ended') && window.__voiceContexts.every(context => context.state === 'closed')))
    holdHandshake = false
    assert.deepEqual(errors, [])
    console.log('PASS: stopping during upstream initialization closes the microphone and releases the model connection')
    await page.setViewportSize({ width: 390, height: 844 })
    await page.getByRole('button', { name: 'Open navigation', exact: true }).waitFor()
    await page.screenshot({ path: path.join(artifacts, 'voice-mobile.png'), fullPage: true, animations: 'disabled' })
    assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('main,section,fieldset,input,textarea,h1')].map(el => ({ tag:el.tagName, width:el.clientWidth, scroll:el.scrollWidth, right:el.getBoundingClientRect().right })).filter(el => el.right > innerWidth+1 || el.scroll > el.width+2)), [])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
    await page.getByRole('textbox', { name: 'Role instructions', exact: true }).fill('Unsaved voice draft')
    await req('/config', 'PUT', { ...input, expected_revision: 1 })
    await page.getByRole('button', { name: 'Save voice settings', exact: true }).click()
    await page.getByRole('alert').filter({ hasText: 'Voice configuration changed' }).waitFor()
    assert.equal(await page.getByRole('textbox', { name: 'Role instructions', exact: true }).inputValue(), 'Unsaved voice draft')
    console.log('PASS: mobile layout and local draft preservation after revision conflict')
    await page.reload(); await start.waitFor(); await until(async () => start.isEnabled()); await start.click()
    await page.getByText('Voice conversation active', { exact: true }).waitFor()
    await page.goto(web + '/voice?dt_workspace=' + scopeB)
    await until(async () => wsServer.clients.size === 0)
    assert.equal(await page.getByRole('checkbox', { name: 'Enable voice in this workspace', exact: true }).isChecked(), false)
    console.log('PASS: navigation closes the old call; workspace settings remain isolated')
    const language = await fetch(origin + '/api/settings/ui', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ language: 'zh' }) })
    assert.equal(language.status, 200)
    await page.evaluate(() => localStorage.removeItem('deeptutor-language'))
    await page.reload()
    await page.getByRole('checkbox', { name: '在当前工作区启用语音', exact: true }).waitFor()
    await page.screenshot({ path: path.join(artifacts, 'voice-zh-mobile.png'), fullPage: true, animations: 'disabled' })
    assert.deepEqual(errors, [])
    console.log('PASS: Chinese voice page labels and controls')
  }
} catch (error) {
  if (page) await page.screenshot({ path: path.join(artifacts, 'failure.png'), fullPage: true }).catch(() => {})
  console.error(logs.slice(-6000)); throw error
} finally {
  await browser?.close(); await stop(frontend); await stop(backend)
  for (const socket of wsServer.clients) socket.terminate()
  await new Promise(resolve => wsServer.close(resolve)); await new Promise(resolve => upstream.close(resolve))
  await rm(temp, { recursive: true, force: true })
}
