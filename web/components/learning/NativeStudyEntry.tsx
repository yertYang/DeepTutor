'use client'
import Link from 'next/link'
import { useTranslation } from 'react-i18next'
export default function NativeStudyEntry() {
  const { t } = useTranslation()
  return (
    <div className="mx-6 my-4 rounded-xl border border-[var(--border)] p-4">
      <h2 className="font-semibold">{t('Study workspace')}</h2>
      <div className="mt-3 flex flex-wrap gap-4">
        {[
          ['deep_research', 'Research'],
          ['deep_question', 'Practice questions'],
          ['mastery_path', 'Learning path'],
        ].map(([kind, label]) => (
          <Link key={kind} className="text-sm underline" href={`/learning/native?kind=${kind}`}>
            {t(label)}
          </Link>
        ))}
      </div>
    </div>
  )
}
