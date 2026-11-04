import assert from 'node:assert/strict'
import { test } from 'node:test'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'

// Node 22.13 is supported and predates default TypeScript type stripping.
const require = createRequire(new URL('../web/package.json', import.meta.url))
const ts = require('typescript')
const source = await readFile(new URL('../web/lib/runtime-features.ts', import.meta.url), 'utf8')
const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } })
const { runtimeFeatureAvailable } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`)

test('native inventory enables implemented previews and gates unknown capabilities', async () => {
  const { modules } = JSON.parse(await readFile(new URL('../rust/server/assets/modules.json', import.meta.url), 'utf8'))
  for (const feature of ['speech_to_text', 'service_preview_stt', 'speech_synthesis', 'voice_math_speak', 'model_diagnostics', 'voice_discovery', 'service_preview_imagegen', 'service_preview_search', 'service_preview_videogen']) {
    assert.equal(runtimeFeatureAvailable(modules, feature), true)
  }
  for (const feature of ['host_execution', 'unknown_feature']) {
    assert.equal(runtimeFeatureAvailable(modules, feature), false)
  }
})

test('features wait for inventory and preserve older deployment capabilities', () => {
  assert.equal(runtimeFeatureAvailable(null, 'speech_synthesis'), false)
  assert.equal(runtimeFeatureAvailable([{ id: 'core' }], 'speech_synthesis'), true)
})
