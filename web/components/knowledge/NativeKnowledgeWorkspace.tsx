'use client'

import { useCallback, useEffect, useState } from 'react'
import { useParams, useRouter } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { BookOpen, Plus, Trash2, Upload } from 'lucide-react'
import { apiFetch, apiUrl } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { decodeResourceSegment, knowledgeBaseRoute } from '@/lib/resource-routes'
import { FeaturePage, PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
import NativeKnowledgeReader from './NativeKnowledgeReader'

type KnowledgeBase = { name: string; statistics: { total_documents: number; total_chunks: number } }
type Policy = { accept: string; max_file_size_bytes: number; ocr_available?: boolean }

export default function NativeKnowledgeWorkspace() {
  const { t } = useTranslation()
  const router = useRouter()
  const params = useParams<{ kbName?: string }>()
  const selected = decodeResourceSegment(params.kbName)
  const [scope] = useState(activeWorkspaceId)
  const [bases, setBases] = useState<KnowledgeBase[]>([])
  const [policy, setPolicy] = useState<Policy | null>(null)
  const [name, setName] = useState('')
  const [files, setFiles] = useState<File[]>([])
  const [creating, setCreating] = useState(false)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [revision, setRevision] = useState(0)
  const url = useCallback(
    (path = '') => apiUrl(scopedUrl(`/api/knowledge-bases${path}`, scope)),
    [scope]
  )
  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      const responses = await Promise.all([
        apiFetch(url('/list'), { cache: 'no-store', signal }),
        apiFetch(url('/supported-file-types'), { signal }),
      ])
      const values = await Promise.all(responses.map(response => response.json()))
      for (let i = 0; i < responses.length; i++)
        if (!responses[i].ok)
          throw new Error(values[i].detail || `Request failed (${responses[i].status})`)
      setBases(values[0])
      setPolicy(values[1])
    },
    [url]
  )
  useEffect(() => {
    const controller = new AbortController()
    void refresh(controller.signal)
      .catch(cause => {
        if (!controller.signal.aborted) setError(cause.message)
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [refresh])
  const upload = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const form = event.currentTarget
    if (!files.length || (!creating && !selected)) return
    setBusy(true)
    setError('')
    try {
      const body = new FormData()
      body.set('rag_provider', 'native_lexical')
      if (creating) body.set('name', name.trim())
      for (const file of files) {
        if (policy && file.size > policy.max_file_size_bytes)
          throw new Error(t('Upload size limit exceeded.'))
        body.append('files', file, file.name)
      }
      const response = await apiFetch(
        url(creating ? '' : `/${encodeURIComponent(selected!)}/upload`),
        { method: 'POST', body }
      )
      const value = await response.json()
      if (!response.ok) throw new Error(value.detail || `Upload failed (${response.status})`)
      const target = creating ? name.trim() : selected!
      setFiles([])
      setName('')
      setCreating(false)
      form.reset()
      await refresh()
      setRevision(current => current + 1)
      router.push(scopedUrl(knowledgeBaseRoute(target), scope))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    if (
      !selected ||
      !window.confirm(
        t('Delete this knowledge base and its source files? Saved notes will be kept.')
      )
    )
      return
    setBusy(true)
    setError('')
    try {
      const response = await apiFetch(url(`/${encodeURIComponent(selected)}`), { method: 'DELETE' })
      if (!response.ok) throw new Error((await response.json()).detail)
      await refresh()
      router.push(scopedUrl('/knowledge-bases', scope))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  return (
    <FeaturePage>
      <PageHeader
        title={t('Knowledge bases')}
        icon={BookOpen}
        description={t(
          'Import documents, find source passages, and save cited notes in this workspace.'
        )}
        action={
          <>
            <button
              className={pageActionClass()}
              disabled={busy || !policy}
              onClick={() => {
                setCreating(value => !value)
                setFiles([])
              }}
            >
              <Plus size={16} />
              {t('Create knowledge base')}
            </button>
            {selected && (
              <button
                className={pageActionClass('secondary')}
                disabled={busy}
                onClick={() => void remove()}
              >
                <Trash2 size={16} />
                {t('Delete')}
              </button>
            )}
          </>
        }
      />
      {error && (
        <p
          role="alert"
          className="mb-4 rounded-lg border border-red-500/40 p-3 text-sm text-red-600"
        >
          {error}
        </p>
      )}
      {loading && <p role="status">{t('Loading...')}</p>}
      <nav aria-label={t('Knowledge bases')} className="mb-4 flex flex-wrap gap-2">
        {bases.map(base => (
          <button
            key={base.name}
            aria-current={base.name === selected ? 'page' : undefined}
            className={pageActionClass(base.name === selected ? undefined : 'secondary')}
            onClick={() => {
              setCreating(false)
              router.push(scopedUrl(knowledgeBaseRoute(base.name), scope))
            }}
          >
            {base.name}
            <span className="text-xs opacity-60">{base.statistics.total_documents}</span>
          </button>
        ))}
      </nav>
      {(creating || selected) && (
        <form
          className="mb-5 space-y-3 rounded-xl border border-[var(--border)] p-4"
          onSubmit={event => void upload(event)}
        >
          {creating && (
            <label className="block text-sm">
              {t('Knowledge base name')}
              <input
                autoFocus
                required
                maxLength={100}
                value={name}
                onChange={event => setName(event.target.value)}
                className="ml-3 max-w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
              />
            </label>
          )}
          <label className="block text-sm">
            {t('Documents')}
            <input
              type="file"
              multiple
              required
              accept={policy?.accept}
              onChange={event => setFiles(Array.from(event.target.files ?? []))}
              className="mt-2 block max-w-full text-sm"
            />
          </label>
          <p className="text-xs text-[var(--muted-foreground)]">
            {t(
              policy?.ocr_available
                ? 'Supports PDF, Office documents, text, PNG and JPEG. Local OCR supports Chinese and English; scanned PDFs are limited to 50 pages.'
                : 'Supports PDF, DOCX, PPTX, XLSX, TXT, Markdown, CSV and JSON. Up to 16 MiB per upload; scanned images require OCR.'
            )}
          </p>
          <button
            type="submit"
            className={pageActionClass()}
            disabled={busy || !files.length || (creating && !name.trim())}
          >
            <Upload size={15} />
            {busy ? t('Uploading...') : t('Import documents')}
          </button>
        </form>
      )}
      {selected && !creating && (
        <div className="min-h-[420px] rounded-xl border border-[var(--border)]">
          <NativeKnowledgeReader
            key={`${scope}:${selected}`}
            kbName={selected}
            revision={revision}
          />
        </div>
      )}
    </FeaturePage>
  )
}
