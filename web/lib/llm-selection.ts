import type { LLMSelection } from '@/features/chat/model/protocol'

export const CONVERSATION_REASONING_EFFORTS = [
  'none',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
  'adaptive',
] as const

/** Restore only safe selection fields from a session or failed-turn snapshot. */
export function readLLMSelection(value: unknown): LLMSelection | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  const profileId = typeof record.profile_id === 'string' ? record.profile_id.trim() : ''
  const modelId = typeof record.model_id === 'string' ? record.model_id.trim() : ''
  if (!profileId || !modelId) return null
  const selection: LLMSelection = { profile_id: profileId, model_id: modelId }
  const effort =
    typeof record.reasoning_effort === 'string' ? record.reasoning_effort.trim().toLowerCase() : ''
  if ((CONVERSATION_REASONING_EFFORTS as readonly string[]).includes(effort)) {
    selection.reasoning_effort = effort
  }
  return selection
}
