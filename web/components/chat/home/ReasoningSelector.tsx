"use client";

import { Brain } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { LLMSelection } from "@/features/chat/model/protocol";
import { sameLLMSelection, type LLMOption } from "@/lib/llm-options";
import { reasoningEffortOptions, reasoningEffortOptionsFromSupportedLevels } from "@/lib/reasoning-effort";

export default function ReasoningSelector({ options, value, activeDefault, disabled, onChange }: {
  options: LLMOption[];
  value: LLMSelection | null;
  activeDefault: LLMSelection | null;
  disabled?: boolean;
  onChange: (value: LLMSelection | null) => void;
}) {
  const { t } = useTranslation();
  const selection = value ?? activeDefault;
  const model = options.find(option => sameLLMSelection(option, selection));
  if (!model || !selection) return null;
  const efforts = model.supported_reasoning_efforts !== undefined
    ? reasoningEffortOptionsFromSupportedLevels(model.supported_reasoning_efforts)
    : reasoningEffortOptions(model.provider, model.model, "");
  if (!efforts.length) return null;
  const selected = selection.reasoning_effort ?? "";
  return (
    <label className="relative inline-flex h-8 min-w-0 shrink-0 items-center gap-1 rounded-lg px-1.5 text-xs text-[var(--muted-foreground)] hover:bg-[var(--muted)]/55">
      <Brain size={15} aria-hidden="true" className="shrink-0" />
      <span className="sr-only">{t("Reasoning effort")}</span>
      <select
        aria-label={t("Reasoning effort")}
        value={selected}
        disabled={disabled}
        onChange={event => onChange({
          profile_id: selection.profile_id,
          model_id: selection.model_id,
          reasoning_effort: (event.target.value || null) as LLMSelection["reasoning_effort"],
        })}
        className="h-8 max-w-[105px] cursor-pointer rounded-md border-0 bg-transparent pr-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-[var(--ring)] disabled:opacity-40 sm:max-w-[145px]"
      >
        {efforts.map(option => <option key={option.value} value={option.value} className="bg-[var(--popover)] text-[var(--foreground)]">
          {t(option.value ? option.label : "Auto")}
        </option>)}
        {selected && !efforts.some(option => option.value === selected) && (
          <option value={selected} disabled>{selected}</option>
        )}
      </select>
    </label>
  );
}
