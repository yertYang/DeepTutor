"use client";

import { useTranslation } from "react-i18next";
import { useRuntimeFeature } from "@/hooks/useRuntimeFeature";
import { notify } from "@/lib/notifications";

import { useVoiceAutoplayPreference } from "@/hooks/useVoiceAutoplay";
import { useVoiceMathSpeakPreference } from "@/hooks/useVoiceMathSpeak";

function PlaybackToggle({
  label,
  description,
  value,
  loading,
  onChange,
}: {
  label: string;
  description: string;
  value: boolean;
  loading: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-6 rounded-xl border border-[var(--border)]/60 bg-[var(--card)]/40 px-5 py-4">
      <div className="min-w-0 flex-1">
        <div className="text-[13.5px] font-medium text-[var(--foreground)]">
          {label}
        </div>
        <p className="mt-1 text-[12px] leading-relaxed text-[var(--muted-foreground)]">
          {description}
        </p>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={value}
        disabled={loading}
        onClick={() => onChange(!value)}
        className={`relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 ${
          value ? "bg-[var(--foreground)]" : "bg-[var(--border)]"
        }`}
        aria-label={label}
      >
        <span
          className={`inline-block h-4 w-4 transform rounded-full bg-[var(--background)] shadow-sm transition-transform ${
            value ? "translate-x-4" : "translate-x-0.5"
          }`}
        />
      </button>
    </div>
  );
}

export function VoicePlaybackPrefs() {
  const { t } = useTranslation();
  const available = useRuntimeFeature("speech_synthesis");
  if (!available)
    return (
      <p role="status" className="mb-8 text-xs text-[var(--muted-foreground)]">
        {t("Speech playback is not available in this runtime.")}
      </p>
    );
  return <AvailableVoicePlaybackPrefs />;
}

function AvailableVoicePlaybackPrefs() {
  const { t } = useTranslation();
  const autoplay = useVoiceAutoplayPreference();
  const supportsMathSpeak = useRuntimeFeature("voice_math_speak");
  const mathSpeak = useVoiceMathSpeakPreference();
  return (
    <section className="mb-8">
      <div className="mb-3">
        <h2 className="text-[15px] font-semibold tracking-tight text-[var(--foreground)]">
          {t("Playback")}
        </h2>
        <p className="mt-1 text-[12.5px] leading-relaxed text-[var(--muted-foreground)]">
          {t("How spoken replies behave in chat.")}
        </p>
      </div>
      <div className="space-y-3">
        <PlaybackToggle
          label={t("Auto-play replies")}
          description={t(
            "Read each assistant reply aloud automatically. You can also toggle this per conversation from the speaker button.",
          )}
          value={autoplay.value}
          loading={autoplay.loading}
          onChange={(next) => {
            void autoplay
              .setValue(next)
              .catch(() =>
                notify(t("Could not save voice preferences."), {
                  tone: "error",
                }),
              );
          }}
        />
        {supportsMathSpeak && <PlaybackToggle
          label={t("Math speak")}
          description={t(
            "Read LaTeX as words — fractions, powers, and Greek letters. Turn this off to keep formulas closer to the written math. Dollar signs are never spoken.",
          )}
          value={mathSpeak.value}
          loading={mathSpeak.loading}
          onChange={(next) => {
            void mathSpeak
              .setValue(next)
              .catch(() =>
                notify(t("Could not save voice preferences."), {
                  tone: "error",
                }),
              );
          }}
        />}
      </div>
    </section>
  );
}
