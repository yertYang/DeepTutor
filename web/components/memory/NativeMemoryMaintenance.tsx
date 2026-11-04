'use client'
import Link from 'next/link'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { apiFetch, apiUrl } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { pageActionClass } from '@/components/layout/FeaturePage'
type Policy = {
  enabled: boolean
  revision: number
  last_scan_at: number | null
  proposal_ids: string[]
  skipped_documents?: number
}
export default function NativeMemoryMaintenance({ blocked }: { blocked: boolean }) {
  const { t } = useTranslation()
  const [scope] = useState(activeWorkspaceId)
  const [policy, setPolicy] = useState<Policy | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const request = useCallback(
    async (body?: unknown, scan = false) => {
      const response = await apiFetch(
        apiUrl(scopedUrl('/api/memory/maintenance' + (scan ? '/scan' : ''), scope)),
        {
          method: scan ? 'POST' : body ? 'PUT' : 'GET',
          headers: { 'content-type': 'application/json' },
          ...(body ? { body: JSON.stringify(body) } : {}),
          cache: 'no-store',
        }
      )
      const value = await response.json()
      if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
      return value as Policy
    },
    [scope]
  )
  useEffect(() => {
    let active = true
    const load = () =>
      void request()
        .then(value => {
          if (active) setPolicy(value)
        })
        .catch(cause => {
          if (active) setError(cause.message)
        })
    load()
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') load()
    }, 15000)
    return () => {
      active = false
      clearInterval(timer)
    }
  }, [request])
  const change = async (body?: unknown, scan = false) => {
    setBusy(true)
    setError('')
    try {
      setPolicy(await request(body, scan))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  return (
    <details className="mb-5 rounded-xl border border-[var(--border)] p-4">
      <summary className="cursor-pointer text-sm font-medium">
        {t('Automatic memory cleanup')}
      </summary>
      <div className="mt-3 space-y-3 text-sm">
        <p className="text-[var(--muted-foreground)]">
          {t(
            'Find identical repeated bullet lines and prepare a proposal. Review it before applying; facts and citations stay in the source.'
          )}
        </p>
        <label className="flex items-start gap-2">
          <input
            type="checkbox"
            checked={policy?.enabled ?? false}
            disabled={!policy || busy}
            onChange={event =>
              void change({ enabled: event.target.checked, expected_revision: policy?.revision })
            }
          />
          {t('Prepare cleanup proposals automatically every minute')}
        </label>
        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            className={pageActionClass('secondary')}
            disabled={blocked || busy || !policy}
            onClick={() => void change(undefined, true)}
          >
            {t('Scan saved memory now')}
          </button>
          <Link href={scopedUrl('/memory/graph', scope)} className="underline">
            {t('Memory graph')}
          </Link>
          {policy?.last_scan_at && (
            <span className="text-xs text-[var(--muted-foreground)]">
              {t('Last scan')}: {new Date(policy.last_scan_at * 1000).toLocaleString()}
            </span>
          )}
        </div>
        {Boolean(policy?.skipped_documents) && (
          <p>{t('Some documents exceed the automatic cleanup size limit. Use manual editing.')}</p>
        )}
        {policy?.proposal_ids.map(id => (
          <Link
            key={id}
            className="block break-words underline"
            href={scopedUrl('/learning/native?artifact_id=' + encodeURIComponent(id), scope)}
          >
            {t('Review cleanup proposal')}
          </Link>
        ))}
        {error && (
          <p role="alert" className="text-red-600">
            {error}
          </p>
        )}
      </div>
    </details>
  )
}
