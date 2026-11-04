'use client'
import Link from 'next/link'
import { useTranslation } from 'react-i18next'
import { FeaturePage, PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
import { scopedUrl } from '@/lib/workspace-scope'
export default function NativeReadingUnavailable() {
  const { t } = useTranslation()
  return (
    <FeaturePage>
      <PageHeader
        title={t('Reading')}
        description={t(
          'Reading conversations are not yet available in the native workspace. Open the library to read and annotate imported documents.'
        )}
      />
      <Link href={scopedUrl('/learning/reading')} className={pageActionClass()}>
        {t('Reading library')}
      </Link>
    </FeaturePage>
  )
}
