'use client'

import dynamic from 'next/dynamic'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useRouter } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { Brain, History, Save } from 'lucide-react'
import { apiFetch } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { PageHeader, pageActionClass } from '@/components/layout/FeaturePage'

import NativeMemoryMaintenance from './NativeMemoryMaintenance'

const Markdown = dynamic(() => import('@/components/common/MarkdownRenderer'), { ssr: false })
const surfaces = ['chat', 'notebook', 'quiz', 'kb', 'book', 'partner', 'cowriter']
const slots = ['recent', 'profile', 'scope', 'preferences']
type Doc = {
  layer: string
  key: string
  content: string
  revision: number
  updated_at: string | null
}
type Summary = Omit<Doc, 'content'> & { exists: boolean; entry_count: number }
type Version = { id: number; created_at: number }

export function nativeMemoryTarget(pathname: string): [string, string] | null {
  const parts = pathname.split('/').filter(Boolean)
  if (parts.length === 1 && parts[0] === 'memory') return ['L3', 'preferences']
  const layer = parts[1]?.toUpperCase()
  const keys = layer === 'L2' ? surfaces : layer === 'L3' ? slots : []
  const key = parts[2] ?? (layer === 'L2' ? 'chat' : 'preferences')
  return parts.length <= 3 && keys.includes(key) ? [layer, key] : null
}

export default function NativeMemoryWorkspace({
  layer,
  docKey,
}: {
  layer: string
  docKey: string
}) {
  const { t } = useTranslation()
  const router = useRouter()
  const [scope] = useState(activeWorkspaceId)
  const draftKey = `deeptutor.native-memory:${scope}:${layer}/${docKey}`
  const [doc, setDoc] = useState<Doc | null>(null)
  const [docs, setDocs] = useState<Summary[]>([])
  const [content, setContent] = useState('')
  const [recovery, setRecovery] = useState<string | null>(null)
  const [versions, setVersions] = useState<Version[] | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [editing, setEditing] = useState(false)
  const latest = useRef({ content: '', revision: 0, dirty: false })
  const request = useCallback(
    async <T,>(path: string, body?: unknown, method = 'POST'): Promise<T> => {
      const response = await apiFetch(
        scopedUrl(`/api/memory${path}`, scope),
        body === undefined
          ? { cache: 'no-store' }
          : {
              method,
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }
      )
      const value = await response.json()
      if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
      return value
    },
    [scope]
  )
  const path = `/doc/${layer}/${docKey}`
  const keepDraft = useCallback(() => {
    if (latest.current.dirty)
      browserStorage.writeRaw('local', draftKey, JSON.stringify(latest.current))
  }, [draftKey])
  const load = useCallback(async () => {
    const [current, overview] = await Promise.all([
      request<Doc>(path),
      request<{ docs: Summary[] }>('/overview'),
    ])
    setDoc(current)
    setDocs(overview.docs)
    setContent(current.content)
    latest.current = { content: current.content, revision: current.revision, dirty: false }
    setRecovery(null)
    const raw = browserStorage.readRaw('local', draftKey)
    if (raw) {
      try {
        const draft = JSON.parse(raw)
        if (typeof draft.content === 'string' && draft.content !== current.content) {
          if (draft.revision === current.revision) {
            latest.current = { content: draft.content, revision: current.revision, dirty: true }
            setContent(draft.content)
            setEditing(true)
          } else setRecovery(draft.content)
        }
      } catch {
        /* Invalid drafts cannot replace persistent memory. */
      }
    }
  }, [request, path, draftKey])
  useEffect(() => {
    let active = true
    void load().catch(err => {
      if (active) setError(String(err))
    })
    return () => {
      active = false
    }
  }, [load])
  useEffect(() => {
    const unloading = (event: BeforeUnloadEvent) => {
      if (latest.current.dirty) {
        keepDraft()
        event.preventDefault()
        event.returnValue = ''
      }
    }
    const switching = () => keepDraft()
    window.addEventListener('beforeunload', unloading)
    window.addEventListener('deeptutor:before-workspace-switch', switching)
    return () => {
      window.removeEventListener('beforeunload', unloading)
      window.removeEventListener('deeptutor:before-workspace-switch', switching)
    }
  }, [keepDraft])
  const change = (value: string) => {
    setContent(value)
    latest.current = { ...latest.current, content: value, dirty: value !== doc?.content }
    if (latest.current.dirty) keepDraft()
    else browserStorage.removeRaw('local', draftKey)
  }
  const perform = async (action: () => Promise<void>) => {
    setBusy(true)
    setError('')
    try {
      await action()
    } catch (err) {
      keepDraft()
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }
  const accept = (result: Doc) => {
    latest.current = { content: result.content, revision: result.revision, dirty: false }
    browserStorage.removeRaw('local', draftKey)
    setDoc(result)
    setContent(result.content)
    setEditing(false)
    setVersions(null)
    setDocs(current =>
      current.map(row =>
        row.layer === layer && row.key === docKey
          ? { ...row, revision: result.revision, updated_at: result.updated_at, exists: true }
          : row
      )
    )
  }
  const navigate = (nextLayer: string, key: string) => {
    keepDraft()
    router.push(scopedUrl(`/memory/${nextLayer.toLowerCase()}/${key}`, scope))
  }
  return (
    <section className="h-full min-h-0 overflow-auto p-4 md:p-6">
      <PageHeader
        icon={Brain}
        title={t('Memory')}
        description={t('Read, edit and restore memory in this workspace.')}
      />
      {error && (
        <div
          role="alert"
          className="my-3 rounded-lg bg-red-500/10 p-3 text-sm text-red-700 dark:text-red-300"
        >
          {error}
        </div>
      )}
      {recovery !== null && (
        <div className="my-3 flex flex-wrap items-center gap-3 rounded-lg border border-[var(--border)] p-3 text-sm">
          <span>{t('A local draft differs from this page.')}</span>
          <button
            className={pageActionClass('secondary')}
            disabled={busy}
            onClick={() => {
              setEditing(true)
              change(recovery)
              setRecovery(null)
            }}
          >
            {t('Use local draft')}
          </button>
        </div>
      )}
      <NativeMemoryMaintenance blocked={latest.current.dirty} />
      <div className="grid gap-5 md:grid-cols-[220px_minmax(0,1fr)]">
        <nav aria-label={t('Memory documents')} className="space-y-4">
          {['L3', 'L2'].map(group => (
            <div key={group}>
              <h2 className="mb-2 text-xs font-semibold text-[var(--muted-foreground)]">{group}</h2>
              <div className="flex flex-wrap gap-1 md:flex-col">
                {docs
                  .filter(row => row.layer === group)
                  .map(row => (
                    <button
                      key={row.key}
                      disabled={busy}
                      aria-current={row.layer === layer && row.key === docKey ? 'page' : undefined}
                      onClick={() => navigate(row.layer, row.key)}
                      className={`rounded-lg px-3 py-2 text-left text-sm hover:bg-[var(--muted)] ${row.layer === layer && row.key === docKey ? 'bg-[var(--muted)] font-semibold' : ''}`}
                    >
                      {t(row.key)}
                      <span className="ml-2 text-xs text-[var(--muted-foreground)]">
                        #{row.revision}
                      </span>
                    </button>
                  ))}
              </div>
            </div>
          ))}
        </nav>
        <div className="min-w-0">
          {!doc ? (
            <p role="status">{t('Loading...')}</p>
          ) : (
            <>
              <div className="mb-3 flex flex-wrap items-center gap-2">
                <h2 className="mr-auto font-semibold">
                  {layer} / {t(docKey)}
                </h2>
                <span role="status" className="text-xs text-[var(--muted-foreground)]">
                  {t(latest.current.dirty ? 'Unsaved changes' : 'Saved')} · #{doc.revision}
                </span>
                <button
                  className={pageActionClass('secondary')}
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      keepDraft()
                      await load()
                    })
                  }
                >
                  {t('Reload page')}
                </button>
                <button
                  className={pageActionClass('secondary')}
                  disabled={busy || latest.current.dirty}
                  onClick={() =>
                    void perform(async () => {
                      if (versions) setVersions(null)
                      else
                        setVersions(
                          (await request<{ versions: Version[] }>(`${path}/versions`)).versions
                        )
                    })
                  }
                >
                  <History size={14} />
                  {t('Version history')}
                </button>
                <button
                  className={pageActionClass('secondary')}
                  disabled={busy}
                  onClick={() => setEditing(value => !value)}
                >
                  {t(editing ? 'Preview' : 'Edit')}
                </button>
                <button
                  className={pageActionClass('secondary')}
                  disabled={busy || latest.current.dirty || !doc?.content.trim()}
                  onClick={() =>
                    router.push(
                      scopedUrl(
                        `/learning/native?kind=memory_consolidate&memory_layer=${layer}&memory_key=${docKey}&revision=${doc?.revision}`,
                        scope
                      )
                    )
                  }
                >
                  {t('Consolidate memory')}
                </button>
                <button
                  className={pageActionClass()}
                  disabled={busy || !latest.current.dirty}
                  onClick={() =>
                    void perform(async () => {
                      accept(
                        await request<Doc>(
                          path,
                          {
                            content: latest.current.content,
                            expected_revision: latest.current.revision,
                          },
                          'PUT'
                        )
                      )
                    })
                  }
                >
                  <Save size={14} />
                  {t('Save')}
                </button>
              </div>
              {versions && (
                <div className="mb-4 flex flex-wrap gap-2 rounded-lg border border-[var(--border)] p-3">
                  {!versions.length && <p className="text-sm">{t('No previous versions.')}</p>}
                  {versions.map(version => (
                    <button
                      key={version.id}
                      className={pageActionClass('secondary')}
                      disabled={busy}
                      onClick={() =>
                        void perform(async () => {
                          accept(
                            await request<Doc>(`${path}/restore/${version.id}`, {
                              expected_revision: latest.current.revision,
                            })
                          )
                        })
                      }
                    >
                      {t('Restore')} #{version.id} ·{' '}
                      {new Date(version.created_at * 1000).toLocaleString()}
                    </button>
                  ))}
                </div>
              )}
              {editing ? (
                <textarea
                  aria-label={t('Memory document')}
                  value={content}
                  disabled={busy}
                  spellCheck={false}
                  onChange={event => change(event.target.value)}
                  className="min-h-[420px] w-full rounded-xl border border-[var(--border)] bg-[var(--background)] p-4 font-mono text-sm leading-6 outline-none focus:border-[var(--ring)]"
                />
              ) : (
                <div className="min-h-[300px] rounded-xl border border-[var(--border)] p-5">
                  {content ? (
                    <Markdown content={content} variant="prose" />
                  ) : (
                    <p className="text-sm text-[var(--muted-foreground)]">
                      {t('This memory document is empty. Click Edit to start writing.')}
                    </p>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  )
}
