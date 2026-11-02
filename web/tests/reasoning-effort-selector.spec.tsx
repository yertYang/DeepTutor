import { useState } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import ReasoningEffortSelector from '@/components/chat/home/ReasoningEffortSelector'
import type { LLMOption } from '@/lib/llm-options'
import type { LLMSelection } from '@/features/chat/model/protocol'

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

const model: LLMOption = {
  profile_id: 'p1',
  model_id: 'm1',
  profile_name: 'Gateway',
  model_name: 'Tutor',
  model: 'gateway-tutor',
  provider: 'custom',
  reasoning_effort: 'max',
  is_active_default: true,
}

function Picker({ option = model }: { option?: LLMOption }) {
  const [selection, setSelection] = useState<LLMSelection | null>(null)
  return (
    <>
      <ReasoningEffortSelector
        options={[option]}
        activeDefault={option}
        value={selection}
        onChange={setSelection}
      />
      <output data-testid="selection">{JSON.stringify(selection)}</output>
    </>
  )
}

it('shows the saved max default, selects a conversation effort, and resets to inheritance', () => {
  render(<Picker />)
  const select = screen.getByRole('combobox', { name: 'Reasoning effort' })
  expect(screen.getByRole('option', { name: 'Default (max)' })).toBeInTheDocument()
  expect(screen.getByRole('option', { name: 'Maximum (max)' })).toBeInTheDocument()
  fireEvent.change(select, { target: { value: 'low' } })
  expect(JSON.parse(screen.getByTestId('selection').textContent!)).toEqual({
    profile_id: 'p1',
    model_id: 'm1',
    reasoning_effort: 'low',
  })
  expect(model.reasoning_effort).toBe('max')
  fireEvent.change(select, { target: { value: '' } })
  expect(JSON.parse(screen.getByTestId('selection').textContent!)).toEqual({
    profile_id: 'p1',
    model_id: 'm1',
  })
})

it('uses the authoritative supported levels and permits clearing an obsolete override', () => {
  const changed = {
    ...model,
    supported_reasoning_efforts: ['low', 'high'],
    reasoning_effort: 'high',
  }
  render(
    <ReasoningEffortSelector
      options={[changed]}
      activeDefault={changed}
      value={{ profile_id: 'p1', model_id: 'm1', reasoning_effort: 'max' }}
      onChange={vi.fn()}
    />
  )
  expect(screen.getByRole('option', { name: 'Unsupported reasoning effort: max' })).toBeDisabled()
  expect(screen.queryByRole('option', { name: 'Maximum (max)' })).not.toBeInTheDocument()
})

it('hides for non-reasoning models and updates its inherited value when switching models', () => {
  const { rerender } = render(
    <Picker option={{ ...model, declared_reasoning: false, reasoning_effort: undefined }} />
  )
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument()
  rerender(<Picker option={{ ...model, model_id: 'm2', reasoning_effort: 'high' }} />)
  expect(screen.getByRole('option', { name: 'Default (high)' })).toBeInTheDocument()
})
