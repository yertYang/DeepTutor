"use client";

import { useTranslation } from "react-i18next";
import { ErrorSurface, LoadingSurface } from "./PageFeedback";

export function WorkspaceLoading() {
  const { t } = useTranslation();
  return <LoadingSurface title={t("Preparing your workspace")} hint={t("Your learning space will be ready in a moment.")} />;
}

export function WorkspaceError({ reset }: { reset: () => void }) {
  const { t } = useTranslation();
  return <ErrorSurface title={t("This page needs another try")} description={t("Try again, or return home to continue learning.")}
    retryLabel={t("Retry")} backLabel={t("Return home")} onRetry={reset} />;
}
