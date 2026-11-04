'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { useRouter } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { FeaturePage, PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { apiFetch, apiUrl } from '@/lib/api'
import { readingChanged, readingRequest, type ReadingItem } from '@/lib/native-reading'

const inputClass =
  'block w-full min-w-0 rounded-lg border border-[var(--border)] bg-[var(--background)] p-2 text-sm'
export default function NativeReadingLibrary({ progressOnly = false }: { progressOnly?: boolean }) {
  const { t } = useTranslation()
  const router = useRouter()
  const [scope] = useState(activeWorkspaceId)
  const [items, setItems] = useState<ReadingItem[]>([])
  const [next, setNext] = useState<number | null>(null)
  const [bases, setBases] = useState<{ name: string }[]>([])
  const [webUrl, setWebUrl] = useState('')
  const webCommand = useRef<{ url: string; id: string } | null>(null)
  const [kb, setKb] = useState('')
  const [files, setFiles] = useState<{ name: string; sha256: string }[]>([])
  const [file, setFile] = useState('')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const load = useCallback(
    async (offset = 0, signal?: AbortSignal) => {
      const v = await readingRequest<{ items: ReadingItem[]; next_offset: number | null }>(
        scope,
        `?offset=${offset}`,
        undefined,
        'GET',
        signal
      )
      setItems(prior => (offset ? [...prior, ...v.items] : v.items))
      setNext(v.next_offset)
    },
    [scope]
  )
  useEffect(() => {
    const controller = new AbortController()
    void Promise.all([
      load(0, controller.signal),
      apiFetch(apiUrl(scopedUrl('/api/knowledge-bases/list', scope)), {
        signal: controller.signal,
      }).then(async r => {
        if (!r.ok) throw new Error(t('Unable to load knowledge bases'))
        setBases(await r.json())
      }),
    ])
      .catch(e => {
        if (!controller.signal.aborted) setError(e.message)
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [load, scope, t])
  useEffect(() => {
    setFiles([])
    setFile('')
    if (!kb) return
    const controller = new AbortController()
    void apiFetch(
      apiUrl(scopedUrl(`/api/knowledge-bases/${encodeURIComponent(kb)}/files`, scope)),
      { signal: controller.signal }
    )
      .then(async r => {
        if (!r.ok) throw new Error(t('Unable to load files'))
        setFiles((await r.json()).files)
      })
      .catch(e => {
        if (!controller.signal.aborted) setError(e.message)
      })
    return () => controller.abort()
  }, [kb, scope, t])
  const perform = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError('')
    try {
      await fn()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <FeaturePage>
      <PageHeader
        title={t(progressOnly ? 'Learning progress' : 'Reading library')}
        description={t(
          'Read imported documents, save your place and keep annotations linked to an original snapshot.'
        )}
        action={
          <Link
            href={scopedUrl('/knowledge-bases', scope)}
            className={pageActionClass('secondary')}
          >
            {t('Import documents')}
          </Link>
        }
      />
      {error && (
        <p role="alert" className="mb-4 break-words text-sm text-red-600">
          {error}
        </p>
      )}
      {!progressOnly && (
        <form
          className="mb-4 flex flex-wrap gap-3 rounded-xl border border-[var(--border)] p-4"
          onSubmit={event => {
            event.preventDefault()
            const url = webUrl.trim()
            if (webCommand.current?.url !== url)
              webCommand.current = { url, id: crypto.randomUUID() }
            void perform(async () => {
              const item = await readingRequest<ReadingItem>(scope, '/web', {
                url,
                command_id: webCommand.current!.id,
              })
              webCommand.current = null
              readingChanged()
              router.push(scopedUrl(`/learning/reading/${item.id}`, scope))
            })
          }}
        >
          <label className="min-w-0 flex-1 text-sm">
            {t('Web page URL')}
            <input
              className={inputClass}
              type="url"
              required
              maxLength={4096}
              disabled={busy}
              value={webUrl}
              onChange={event => setWebUrl(event.target.value)}
              placeholder="https://example.com/article"
            />
          </label>
          <button className={`${pageActionClass()} self-end`} disabled={busy || !webUrl.trim()}>
            {t(busy ? 'Importing…' : 'Save web page')}
          </button>
          <p className="w-full text-xs text-[var(--muted-foreground)]">
            {t(
              'Save a public HTML or text page for offline reading. Pages requiring login or JavaScript should be saved in your browser and imported as a document.'
            )}
          </p>
        </form>
      )}
      {!progressOnly && (
        <form
          className="mb-6 grid gap-3 rounded-xl border border-[var(--border)] p-4 sm:grid-cols-2"
          onSubmit={e => {
            e.preventDefault()
            const selected = files.find(f => f.name === file)
            if (!selected) return
            void perform(async () => {
              const v = await readingRequest<ReadingItem>(scope, '', {
                kb,
                path: file,
                hash: selected.sha256,
              })
              readingChanged()
              router.push(scopedUrl(`/learning/reading/${v.id}`, scope))
            })
          }}
        >
          <label className="text-sm">
            {t('Knowledge base')}
            <select
              aria-label={t('Knowledge base')}
              className={inputClass}
              value={kb}
              onChange={e => setKb(e.target.value)}
              required
              disabled={busy}
            >
              <option value="">{t('Select a knowledge base')}</option>
              {bases.map(b => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
          <label className="text-sm">
            {t('Document')}
            <select
              aria-label={t('Document')}
              className={inputClass}
              value={file}
              onChange={e => setFile(e.target.value)}
              required
              disabled={busy || !files.length}
            >
              <option value="">{t('Select a document')}</option>
              {files.map(f => (
                <option key={f.name} value={f.name}>
                  {f.name}
                </option>
              ))}
            </select>
          </label>
          <p className="text-xs text-[var(--muted-foreground)] sm:col-span-2">
            {t(
              'The reading copy stays available if you replace or remove the knowledge base file. A new file version opens a separate reading record.'
            )}
          </p>
          <div>
            <button className={pageActionClass()} disabled={busy || !file}>
              {t('Add to reading library')}
            </button>
          </div>
        </form>
      )}
      {loading ? (
        <p role="status">{t('Loading...')}</p>
      ) : !items.length ? (
        <p>{t('No reading documents yet')}</p>
      ) : (
        <ul className="grid gap-3 sm:grid-cols-2">
          {items.map(item => (
            <li key={item.id} className="min-w-0 rounded-xl border border-[var(--border)] p-4">
              <Link
                className="break-words font-medium underline"
                href={scopedUrl(`/learning/reading/${item.id}`, scope)}
              >
                {item.title}
              </Link>
              <p className="mt-2 break-all text-xs text-[var(--muted-foreground)]">
                {item.kb} / {item.path}
              </p>
              <p className="mt-3 text-sm">
                {item.progress.completed
                  ? t('Finished reading')
                  : t('Saved at line {{line}} of {{total}}', {
                      line: item.progress.line,
                      total: item.line_count,
                    })}
              </p>
              <p className="mt-1 text-xs text-[var(--muted-foreground)]">
                {new Date(
                  Math.max(item.updated_at, item.progress.updated_at) * 1000
                ).toLocaleString()}
              </p>
            </li>
          ))}
        </ul>
      )}
      {next !== null && (
        <button
          disabled={busy}
          className={`${pageActionClass('secondary')} mt-4`}
          onClick={() => void perform(() => load(next))}
        >
          {t('Load more')}
        </button>
      )}
      <button
        disabled={busy}
        className={`${pageActionClass('secondary')} mt-4 ml-2`}
        onClick={() => void perform(() => load())}
      >
        {t('Refresh')}
      </button>
    </FeaturePage>
  )
}
