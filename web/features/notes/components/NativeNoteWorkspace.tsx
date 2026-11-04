'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { useRouter } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { apiFetch } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { notifyCoWriterChanged } from '@/lib/co-writer-events'
import type { NoteBlock, NotePage, NoteSummary } from '@/lib/notes-api'
import NotePageHead from './NotePageHead'
import NotePageTree from './NotePageTree'
import NativeNoteEditor, { nativeBlocksEditable } from './NativeNoteEditor'
import type { NoteSaveStatus } from './NoteTopBar'
import './note-workspace.css'

type Draft = Pick<NotePage, 'title' | 'blocks' | 'content' | 'revision' | 'icon' | 'cover' | 'parent_id' | 'pinned'>
type Version = { revision: number; updated_at: number; title: string }
const editablePage = (page: NotePage) => nativeBlocksEditable(page.blocks) &&
  new TextEncoder().encode(page.content).byteLength <= 1024 * 1024

export default function NativeNoteWorkspace({ docId }: { docId: string }) {
  const { t } = useTranslation()
  const router = useRouter()
  // Freeze the scope so delayed saves cannot cross a workspace switch.
  const [scope] = useState(activeWorkspaceId)
  const key = `deeptutor.native-note:${scope}:${docId}`
  const [page, setPage] = useState<NotePage | null>(null)
  const [pages, setPages] = useState<NoteSummary[]>([])
  const [versions, setVersions] = useState<Version[] | null>(null)
  const [recovery, setRecovery] = useState<Draft | null>(null)
  const [status, setStatus] = useState<NoteSaveStatus>('Saved')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [epoch, setEpoch] = useState(0)
  const latest = useRef<NotePage | null>(null)
  const serial = useRef(0)
  const saved = useRef(0)
  const inflight = useRef<Promise<void> | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const request = useCallback(async <T,>(path: string, body?: unknown, method = 'POST'): Promise<T> => {
    const response = await apiFetch(scopedUrl(`/api/documents${path}`, scope), body === undefined ? { cache: 'no-store' } : {
      method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    })
    const value = await response.json()
    if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
    return value
  }, [scope])
  const keepDraft = useCallback(() => {
    if (latest.current) browserStorage.writeRaw('local', key, JSON.stringify(latest.current))
  }, [key])
  const refresh = useCallback(async () => { setPages((await request<{ documents: NoteSummary[] }>('')).documents) }, [request])
  const flush = useCallback(async () => {
    if (inflight.current) await inflight.current
    if (!latest.current || saved.current === serial.current) return
    const save = async () => {
      while (latest.current && saved.current !== serial.current) {
        const snapshot = latest.current
        const at = serial.current
        setStatus('Saving…')
        const body = { title: snapshot.title, icon: snapshot.icon, cover: snapshot.cover,
          parent_id: snapshot.parent_id, pinned: snapshot.pinned, expected_revision: snapshot.revision,
          ...(editablePage(snapshot)
            ? snapshot.blocks ? { blocks: snapshot.blocks } : { content: snapshot.content }
            : {}) }
        const result = await request<NotePage>(`/${docId}`, body, 'PUT')
        latest.current = { ...latest.current, revision: result.revision, content: result.content }
        saved.current = at
        setPage(latest.current)
        keepDraft()
      }
      browserStorage.removeRaw('local', key)
      setStatus('Saved')
      setError('')
      notifyCoWriterChanged()
      void refresh().catch(err => setError(String(err)))
    }
    inflight.current = save()
    try { await inflight.current } catch (err) {
      keepDraft(); setStatus('Save failed'); setError(err instanceof Error ? err.message : String(err)); throw err
    } finally { inflight.current = null }
  }, [request, docId, key, keepDraft, refresh])
  const change = (patch: Partial<NotePage>) => {
    if (!latest.current) return
    latest.current = { ...latest.current, ...patch }
    serial.current++
    setPage(latest.current)
    setStatus('Unsaved changes')
    keepDraft()
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => { void flush().catch(() => {}) }, 900)
  }
  useEffect(() => {
    let active = true
    void request<NotePage>(`/${docId}`).then(doc => {
      if (!active) return
      const raw = browserStorage.readRaw('local', key)
      if (raw) {
        try {
          const draft: Draft = JSON.parse(raw)
          if (typeof draft.title === 'string' && (draft.blocks === null || Array.isArray(draft.blocks)) && typeof draft.content === 'string') {
            if (draft.revision === doc.revision) {
              doc = { ...doc, title: draft.title, blocks: draft.blocks, content: draft.content,
                ...(typeof draft.icon === 'string' ? { icon: draft.icon } : {}),
                ...(typeof draft.cover === 'string' ? { cover: draft.cover } : {}),
                ...(draft.parent_id === null || typeof draft.parent_id === 'string' ? { parent_id: draft.parent_id } : {}),
                ...(typeof draft.pinned === 'boolean' ? { pinned: draft.pinned } : {}),
              }; serial.current++
            }
            else setRecovery(draft)
          }
        } catch { /* Invalid local data must never overwrite a saved page. */ }
      }
      latest.current = doc; setPage(doc)
      if (serial.current !== saved.current) void flush().catch(() => {})
      void refresh().catch(err => setError(String(err)))
    }).catch(err => { if (active) setError(String(err)) })
    return () => { active = false; if (timer.current) clearTimeout(timer.current) }
  }, [docId, key, request, refresh, flush])
  useEffect(() => {
    const switching = (event: Event) => { (event as CustomEvent<Promise<void>[]>).detail.push(flush()) }
    const unloading = (event: BeforeUnloadEvent) => {
      if (serial.current !== saved.current) { keepDraft(); event.preventDefault(); event.returnValue = '' }
    }
    window.addEventListener('deeptutor:before-workspace-switch', switching)
    window.addEventListener('beforeunload', unloading)
    return () => {
      window.removeEventListener('deeptutor:before-workspace-switch', switching)
      window.removeEventListener('beforeunload', unloading)
    }
  }, [flush, keepDraft])
  const perform = async (fn: () => Promise<void>) => {
    setBusy(true)
    try { await flush(); await fn() } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
    finally { setBusy(false) }
  }
  const navigate = (id?: string) => { void perform(async () => { router.push(scopedUrl(id ? `/co-writer/${id}` : '/co-writer', scope)) }) }
  const create = async (parent_id: string | null, draft?: Draft) => {
    const created = await request<NotePage>('', { title: draft?.title ?? '', parent_id,
      ...(draft?.blocks ? { blocks: draft.blocks } : draft ? { content: draft.content } : { blocks: [] }) })
    notifyCoWriterChanged()
    router.push(scopedUrl(`/co-writer/${created.id}`, scope))
  }
  const download = async (format: 'md' | 'json') => {
    const response = await apiFetch(scopedUrl(`/api/documents/${docId}/export?format=${format}`, scope))
    if (!response.ok) throw new Error(`Request failed (${response.status})`)
    const url = URL.createObjectURL(await response.blob())
    const link = document.createElement('a'); link.href = url; link.download = `page-${docId}.${format}`; link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }
  const editable = page && editablePage(page)
  return <div className="dt-note-workspace dt-native-note-workspace">
    <header className="dt-note-topbar dt-native-note-topbar">
      <button className="dt-note-button" disabled={busy} onClick={() => navigate()}>{t('All pages')}</button>
      <span role="status" className="flex-1">{t(status)}</span>
      <button className="dt-note-button" disabled={busy || !editable || Boolean(recovery)} onClick={() => void perform(async () => {
        await flush()
        const source = latest.current
        if (source) router.push(scopedUrl(`/learning/native?kind=note_ai&document_id=${encodeURIComponent(docId)}&revision=${source.revision}`, scope))
      })}>{t('AI edit')}</button>
      <button className="dt-note-button" disabled={busy || !page} onClick={() => void perform(async () => {
        if (versions) setVersions(null)
        else setVersions((await request<{ versions: Version[] }>(`/${docId}/versions`)).versions)
      })}>{t('Version history')}</button>
      <button className="dt-note-button" disabled={busy || !page} onClick={() => void perform(() => download('md'))}>{t('Export Markdown')}</button>
      <button className="dt-note-button" disabled={busy || !page} onClick={() => void perform(() => download('json'))}>{t('Export JSON')}</button>
    </header>
    {error && <div className="dt-note-banner is-error" role="alert"><span className="flex-1">{error}</span>
      <button className="dt-note-button" onClick={() => latest.current ? void perform(async () => {}) : window.location.reload()}>{t('Retry')}</button>
      <button className="dt-note-button" onClick={() => window.location.reload()}>{t('Reload page')}</button>
    </div>}
    {recovery && <div className="dt-note-banner"><span className="flex-1">{t('A local draft differs from this page.')}</span>
      <button className="dt-note-button" disabled={busy} onClick={() => void perform(async () => {
        await create(null, recovery); browserStorage.removeRaw('local', key); setRecovery(null)
      })}>{t('Save draft as a new page')}</button></div>}
    {versions && <div className="dt-note-banner flex-wrap" aria-label={t('Version history')}>
      {versions.map(version => <button className="dt-note-button" key={version.revision} disabled={busy || version.revision === page?.revision}
        onClick={() => void perform(async () => {
          const result = await request<NotePage>(`/${docId}/versions/${version.revision}/restore`, { expected_revision: latest.current?.revision })
          latest.current = result; setPage(result); setEpoch(v => v + 1); setVersions(null); notifyCoWriterChanged(); await refresh()
        })}>{t('Restore')} #{version.revision} · {new Date(version.updated_at * 1000).toLocaleString()}</button>)}
    </div>}
    <div className="dt-note-body">
      <NotePageTree pages={pages} currentId={docId} busy={busy} onNavigate={(event, id) => { event.preventDefault(); navigate(id) }}
        onCreate={parent => void perform(() => create(parent))} />
      <main className="dt-note-main">
        {!page && !error ? <p role="status" className="p-6">{t('Loading pages…')}</p> : null}
        {page && <div className="dt-note-document">
          <NotePageHead page={page} disabled={busy} onTitle={title => change({ title })} onMetadata={change} />
          <label className="flex items-center gap-2 mb-4 text-sm">{t('Move to')}
            <select className="dt-note-button" value={page.parent_id ?? ''} disabled={busy} onChange={event => change({ parent_id: event.target.value || null })}>
              <option value="">{t('All pages')}</option>
              {pages.filter(p => p.id !== docId).map(p => <option key={p.id} value={p.id}>{p.title}</option>)}
            </select>
          </label>
          {editable ? <NativeNoteEditor key={`${docId}:${epoch}`} page={page} disabled={busy} onChange={blocks => change({ blocks })} /> : <>
            <p className="dt-note-banner">{t('This page contains blocks that are not yet editable here. You can read and export the original page.')}</p>
            <pre className="whitespace-pre-wrap break-words p-4">{page.content}</pre>
          </>}
        </div>}
      </main>
    </div>
  </div>
}
