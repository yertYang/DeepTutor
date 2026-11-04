"use client";

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorSurface, LoadingSurface } from "@/components/common/PageFeedback";

interface SessionLoadingViewProps {
  onCancel?: () => void;
  failed?: boolean;
  onRetry?: () => void;
}

/** Keep the conversation layout recognizable while its durable history loads. */
export default function SessionLoadingView({ onCancel, failed = false, onRetry }: SessionLoadingViewProps) {
  const { t } = useTranslation();
  const [showHint, setShowHint] = useState(false);
  useEffect(() => {
    if (failed) return;
    const timer = setTimeout(() => setShowHint(true), 8000);
    return () => clearTimeout(timer);
  }, [failed]);

  if (failed) return <ErrorSurface
    title={t("Failed to load session")}
    description={t("Try again, or return home to continue learning.")}
    retryLabel={t("Retry")} backLabel={t(onCancel ? "Cancel" : "Return home")}
    onRetry={onRetry} onBack={onCancel}
  />;
  return <LoadingSurface title={t("Loading conversation")}
    hint={showHint ? t("Still loading…") : undefined}
    cancelLabel={t("Cancel")} onCancel={onCancel}
  />;
}
