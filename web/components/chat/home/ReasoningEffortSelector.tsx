'use client'

import { Brain } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { LLMSelection } from '@/features/chat/model/protocol'
import { sameLLMSelection, type LLMOption } from '@/lib/llm-options'
import {
  reasoningEffortOptions,
  reasoningEffortOptionsFromSupportedLevels,
} from '@/lib/reasoning-effort'

export default function ReasoningEffortSelector({
  options,
  activeDefault,
  value,
  disabled = false,
  onChange,
}: {
  options: LLMOption[]
  activeDefault: LLMSelection | null
  value: LLMSelection | null
  disabled?: boolean
  onChange: (selection: LLMSelection) => void
}) {
  const { t } = useTranslation()
  const selection = value ?? activeDefault
  const model = options.find(option => sameLLMSelection(option, selection))
  if (!selection || !model) return null
  const effort = selection.reasoning_effort || ''
  const choices =
    model.supported_reasoning_efforts !== undefined
      ? reasoningEffortOptionsFromSupportedLevels(model.supported_reasoning_efforts)
      : reasoningEffortOptions(
          model.provider,
          model.model,
          model.reasoning_effort || '',
          model.declared_reasoning ?? (model.reasoning_effort ? true : undefined)
        )
  if ((!choices.length || model.declared_reasoning === false) && !effort) return null
  const unsupported = effort && !choices.some(choice => choice.value === effort)

  return (
    <label
      className="inline-flex h-8 shrink-0 items-center gap-1 rounded-lg px-2 text-[12px] text-[var(--muted-foreground)] hover:bg-[var(--muted)]/55"
      title={t("Auto inherits the selected model's saved reasoning effort.")}
    >
      <Brain size={14} aria-hidden="true" />
      <select
        aria-label={t('Reasoning effort')}
        className="max-w-[140px] cursor-pointer bg-transparent py-1 text-[12px] outline-none disabled:cursor-not-allowed disabled:opacity-40"
        value={effort}
        disabled={disabled}
        onChange={event => {
          const next: LLMSelection = {
            profile_id: selection.profile_id,
            model_id: selection.model_id,
          }
          if (event.target.value) next.reasoning_effort = event.target.value
          onChange(next)
        }}
      >
        <option value="">
          {t('Default')}
          {model.reasoning_effort ? ` (${model.reasoning_effort})` : ''}
        </option>
        {choices
          .filter(choice => choice.value)
          .map(choice => (
            <option key={choice.value} value={choice.value}>
              {t(choice.label)} ({choice.value})
            </option>
          ))}
        {unsupported && (
          <option value={effort} disabled>
            {t('Unsupported reasoning effort')}: {effort}
          </option>
        )}
      </select>
    </label>
  )
}
