import assert from 'node:assert/strict'
import test from 'node:test'
import { readLLMSelection } from '../lib/llm-selection'
import { buildStartTurnInput } from '../features/chat/controllers/buildStartTurnInput'

test('session restoration and all capability start requests retain the reasoning override', () => {
  for (const effort of ['max', 'low', 'none', 'adaptive']) {
    const stored = { profile_id: 'p1', model_id: 'm1', reasoning_effort: effort }
    const restored = readLLMSelection(stored)
    assert.deepEqual(restored, stored)
    for (const capability of ['chat', 'deep_solve', 'video_lesson', 'visualize']) {
      const request = buildStartTurnInput({ content: 'hello', capability, llmSelection: restored })
      assert.deepEqual(request.llm_selection, stored)
    }
  }
})

test('restoring old sessions inherits model defaults and discards unsafe selection fields', () => {
  assert.deepEqual(
    readLLMSelection({ profile_id: ' p1 ', model_id: 'm1', api_key: 'not-retained' }),
    { profile_id: 'p1', model_id: 'm1' }
  )
  assert.deepEqual(
    readLLMSelection({ profile_id: 'p1', model_id: 'm1', reasoning_effort: 'invalid' }),
    { profile_id: 'p1', model_id: 'm1' }
  )
  assert.equal(readLLMSelection({ profile_id: 'p1' }), null)
  assert.equal(readLLMSelection([]), null)
  assert.equal(readLLMSelection(null), null)
})
