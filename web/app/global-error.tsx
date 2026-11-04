"use client";

import { PRODUCT_NAME } from "@/lib/product";

import { useSyncExternalStore } from "react";
import { MessageCircle } from "lucide-react";
import { ErrorSurface } from "@/components/common/PageFeedback";
import styles from "@/components/common/PageFeedback.module.css";
import { browserStorage } from "@/shared/storage";

// Root failures cannot depend on AppShellContext or an initialized i18n provider.
function language() {
  try { return browserStorage.readRaw("local", "deeptutor-language") || navigator.language; }
  catch { return navigator.language; }
}
const subscribe = () => () => {};
const BRAND_NAME = PRODUCT_NAME;

export default function GlobalError({ reset }: { reset: () => void }) {
  const locale = useSyncExternalStore(subscribe, language, () => "en");
  const chinese = locale.toLowerCase().startsWith("zh");
  return (
    <html lang={chinese ? "zh" : "en"}>
      <body className={styles.standalone}>
        <header className={styles.standaloneHeader}>
          <MessageCircle size={24} strokeWidth={1.6} aria-hidden="true" />{BRAND_NAME}
        </header>
        <ErrorSurface
          title={chinese ? "暂时没能打开这个页面" : "This page needs another try"}
          description={chinese ? "请重试，或返回首页继续学习。" : "Try again, or return home to continue learning."}
          retryLabel={chinese ? "重新加载" : "Reload"}
          backLabel={chinese ? "返回首页" : "Return home"}
          onRetry={reset}
        />
      </body>
    </html>
  );
}
