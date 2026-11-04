"use client";

import { MessageCircle, RotateCw, ArrowLeft } from "lucide-react";
import styles from "./PageFeedback.module.css";

/** Provider-free presentation also used when the root layout itself fails. */
export function LoadingSurface({ title, hint, cancelLabel, onCancel }: {
  title: string; hint?: string; cancelLabel?: string; onCancel?: () => void;
}) {
  return (
    <section className={styles.surface} aria-busy="true" aria-label={title}>
      <div className={styles.loading}>
        <div className={styles.intro}>
          <div className={styles.mark} aria-hidden="true"><MessageCircle size={23} strokeWidth={1.6} /></div>
          <div role="status"><p className={styles.title}>{title}</p>{hint && <p className={styles.hint}>{hint}</p>}</div>
        </div>
        <div aria-hidden="true">
          <div className={styles.thread}>
            <div className={`${styles.skeleton} ${styles.question}`} />
            <div className={styles.answer}>
              <div className={`${styles.skeleton} ${styles.line}`} />
              <div className={`${styles.skeleton} ${styles.line}`} style={{ width: "92%" }} />
              <div className={`${styles.skeleton} ${styles.line}`} style={{ width: "62%" }} />
            </div>
          </div>
          <div className={styles.composer}>
            <div className={`${styles.skeleton} ${styles.line}`} style={{ width: "38%" }} />
            <div className={styles.composerTools}>
              <div className={`${styles.skeleton} ${styles.chip}`} />
              <div className={`${styles.skeleton} ${styles.chip}`} style={{ width: 52 }} />
              <div className={`${styles.skeleton} ${styles.send}`} />
            </div>
          </div>
        </div>
        {onCancel && <button type="button" className={`${styles.button} ${styles.cancel}`} onClick={onCancel}>{cancelLabel}</button>}
      </div>
    </section>
  );
}

export function ErrorSurface({ title, description, retryLabel, backLabel, onRetry, onBack, backHref = "/" }: {
  title: string; description: string; retryLabel: string; backLabel: string;
  onRetry?: () => void; onBack?: () => void; backHref?: string;
}) {
  return (
    <section className={`${styles.surface} ${styles.error}`}>
      <div className={styles.card}>
        <div className={styles.mark} aria-hidden="true"><MessageCircle size={26} strokeWidth={1.6} /></div>
        <div role="alert">
          <h2 className={styles.title}>{title}</h2>
          <p className={styles.hint}>{description}</p>
        </div>
        <div className={styles.actions}>
          {onRetry && <button type="button" className={`${styles.button} ${styles.primary}`} onClick={onRetry}>
            <RotateCw size={14} aria-hidden="true" />{retryLabel}
          </button>}
          {onBack ? <button type="button" className={styles.button} onClick={onBack}>
            <ArrowLeft size={14} aria-hidden="true" />{backLabel}
          </button> : <a className={styles.button} href={backHref}><ArrowLeft size={14} aria-hidden="true" />{backLabel}</a>}
        </div>
      </div>
    </section>
  );
}
