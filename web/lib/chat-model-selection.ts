import type { LLMSelection } from "@/features/chat/model/protocol";

const EFFORTS = new Set(["none", "minimal", "low", "medium", "high", "xhigh", "max", "adaptive"]);

/** Restore both the model reference and its conversation-specific override. */
export function parseLLMSelection(value: unknown): LLMSelection | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const profile = typeof record.profile_id === "string" ? record.profile_id.trim() : "";
  const model = typeof record.model_id === "string" ? record.model_id.trim() : "";
  if (!profile || !model) return null;
  const effort = typeof record.reasoning_effort === "string" ? record.reasoning_effort.trim().toLowerCase() : "";
  return {
    profile_id: profile,
    model_id: model,
    ...(EFFORTS.has(effort) ? { reasoning_effort: effort as NonNullable<LLMSelection["reasoning_effort"]> } : {}),
  };
}
