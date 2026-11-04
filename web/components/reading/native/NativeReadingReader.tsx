'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { useRouter } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { FeaturePage, PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { notifyCoWriterChanged } from '@/lib/co-writer-events'
import {
  readingChanged,
  readingRequest,
  readingUrl,
  type AnnotationDraft,
  type ReadingAnnotation,
  type ReadingItem,
  type ReadingProgress,
} from '@/lib/native-reading'

const field = 'min-w-0 rounded-lg border border-[var(--border)] bg-[var(--background)] p-2 text-sm'
const colors: Record<string, string> = {
  yellow: 'border-yellow-500/60 bg-yellow-500/10',
  green: 'border-green-500/60 bg-green-500/10',
  blue: 'border-blue-500/60 bg-blue-500/10',
  pink: 'border-pink-500/60 bg-pink-500/10',
}
const PAGE = 200
export default function NativeReadingReader({ itemId }: { itemId: string }) {
  const { t } = useTranslation()
  const router = useRouter()
  const [scope] = useState(activeWorkspaceId)
  const key = `lorevia.reading-draft:${scope}:${itemId}`
  const base = '/' + encodeURIComponent(itemId)
  const request = useCallback(
    <T,>(path = '', body?: unknown, method = 'POST', signal?: AbortSignal) =>
      readingRequest<T>(scope, base + path, body, method, signal),
    [scope, base]
  )
  const [item, setItem] = useState<ReadingItem | null>(null)
  const [lines, setLines] = useState<string[]>([])
  const [annotations, setAnnotations] = useState<ReadingAnnotation[]>([])
  const [draft, setDraft] = useState<AnnotationDraft | null>(null)
  const currentDraft = useRef<AnnotationDraft | null>(null)
  const [title, setTitle] = useState('')
  const [line, setLine] = useState(1)
  const [completed, setCompleted] = useState(false)
  const [offset, setOffset] = useState(0)
  const [jump, setJump] = useState(1)
  const [query, setQuery] = useState('')
  const [match, setMatch] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [showPosition, setShowPosition] = useState(false)
  const [noteLinks, setNoteLinks] = useState<Record<string, string>>({})
  const root = useRef<HTMLDivElement>(null)
  const updateDraft = useCallback(
    (v: AnnotationDraft | null) => {
      currentDraft.current = v
      setDraft(v)
      if (v) {
        if (!browserStorage.writeRaw('session', key, JSON.stringify(v)))
          setError(
            t('Could not keep the annotation draft in browser storage. Save it before leaving.')
          )
      } else browserStorage.removeRaw('session', key)
    },
    [key, t]
  )
  const load = useCallback(
    async (signal?: AbortSignal, initial = false) => {
      const [v, source, a] = await Promise.all([
        request<ReadingItem>('', undefined, 'GET', signal),
        request<{ content: string }>('/source', undefined, 'GET', signal),
        request<{ annotations: ReadingAnnotation[] }>('/annotations', undefined, 'GET', signal),
      ])
      if (signal?.aborted) return
      setItem(v)
      setLines(source.content.split('\n'))
      setAnnotations(a.annotations)
      setTitle(v.title)
      setLine(v.progress.line)
      setCompleted(v.progress.completed)
      if (initial) {
        const anchor = /^#L(\d+)$/.exec(window.location.hash)
        const target = anchor
          ? Math.min(v.line_count, Math.max(1, Number(anchor[1])))
          : v.progress.line
        setOffset(Math.floor((target - 1) / PAGE) * PAGE)
        setJump(target)
        const raw = browserStorage.readRaw('session', key)
        if (raw) {
          try {
            const d = JSON.parse(raw) as AnnotationDraft
            if (
              d.source_id === v.source_id &&
              typeof d.command_id === 'string' &&
              typeof d.note === 'string' &&
              typeof d.color === 'string' &&
              d.color in colors &&
              Number.isInteger(d.start_line) &&
              Number.isInteger(d.end_line) &&
              d.start_line >= 1 &&
              d.end_line >= d.start_line &&
              d.end_line <= v.line_count &&
              (!d.id || (typeof d.id === 'string' && Number.isInteger(d.expected_revision)))
            ) {
              currentDraft.current = d
              setDraft(d)
              setNotice(t('Annotation draft restored'))
            }
          } catch {
            /* Invalid browser data cannot overwrite a stored annotation. */
          }
        }
        if (target > 1)
          requestAnimationFrame(() => {
            const row = document.getElementById(`L${target}`)
            const pane = root.current
            if (row && pane) pane.scrollTop += row.getBoundingClientRect().top - pane.getBoundingClientRect().top - 16
          })
      }
    },
    [key, request, t]
  )
  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal, true).catch(e => {
      if (!controller.signal.aborted) setError(e.message)
    })
    return () => controller.abort()
  }, [load])
  useEffect(() => {
    const keep = () =>
      !currentDraft.current ||
      browserStorage.writeRaw('session', key, JSON.stringify(currentDraft.current))
    const switching = (e: Event) => {
      if (!keep())
        (e as CustomEvent<Promise<void>[]>).detail.push(
          Promise.reject(new Error('Draft storage unavailable'))
        )
    }
    const unloading = (e: BeforeUnloadEvent) => {
      if (currentDraft.current) {
        keep()
        e.preventDefault()
        e.returnValue = ''
      }
    }
    window.addEventListener('deeptutor:before-workspace-switch', switching)
    window.addEventListener('beforeunload', unloading)
    return () => {
      window.removeEventListener('deeptutor:before-workspace-switch', switching)
      window.removeEventListener('beforeunload', unloading)
    }
  }, [key])
  const perform = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError('')
    setNotice('')
    try {
      await fn()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }
  const newDraft = (start = jump, end = start) => {
    if (!item) return
    updateDraft({
      command_id: crypto.randomUUID(),
      source_id: item.source_id,
      start_line: start,
      end_line: end,
      note: '',
      color: 'yellow',
    })
  }
  const reveal = (target: number) => {
    const bounded = Math.min(lines.length, Math.max(1, target))
    setOffset(Math.floor((bounded - 1) / PAGE) * PAGE)
    setJump(bounded)
    requestAnimationFrame(() =>
      document.getElementById(`L${bounded}`)?.scrollIntoView({ block: 'center' })
    )
  }
  const selectText = () => {
    if (draft || busy) return
    const selection = window.getSelection()
    const findLine = (node: Node | null) => {
      const el = node instanceof Element ? node : node?.parentElement
      const lineEl = el?.closest<HTMLElement>('[data-reading-line]')
      return lineEl && root.current?.contains(lineEl) ? Number(lineEl.dataset.readingLine) : null
    }
    const start = findLine(selection?.anchorNode ?? null),
      end = findLine(selection?.focusNode ?? null)
    if (selection && !selection.isCollapsed && start && end)
      newDraft(Math.min(start, end), Math.max(start, end))
  }
  const selectedQuote = draft ? lines.slice(draft.start_line - 1, draft.end_line).join('\n') : ''
  const progressDirty =
    item && (line !== item.progress.line || completed !== item.progress.completed)
  return (
    <FeaturePage>
      <PageHeader
        title={item?.title || t('Reading')}
        description={t(
          'Annotations refer to this saved source snapshot. Check extracted text against the original when needed.'
        )}
        action={
          <>
            <Link
              className={pageActionClass('secondary')}
              href={scopedUrl('/learning/reading', scope)}
            >
              {t('Reading library')}
            </Link>
            {item && (
              <>
                <a
                  className={pageActionClass('secondary')}
                  href={readingUrl(base + '/original', scope)}
                  download
                >
                  {t('Download original')}
                </a>
                <a
                  className={pageActionClass('secondary')}
                  href={readingUrl(base + '/export', scope)}
                  download
                >
                  {t('Export reading record')}
                </a>
              </>
            )}
          </>
        }
      />
      {error && (
        <p
          role="alert"
          className="mb-3 break-words rounded-lg border border-red-500/40 p-3 text-sm text-red-600"
        >
          {error}
        </p>
      )}
      <p role="status" className="mb-3 text-sm text-[var(--muted-foreground)]">
        {notice}
      </p>
      <button
        className={`${pageActionClass('secondary')} mb-4`}
        disabled={busy}
        onClick={() => void perform(() => load())}
      >
        {t('Reload saved data')}
      </button>
      {!item ? (
        !error && <p>{t('Loading...')}</p>
      ) : (
        <>
          <details className="mb-4 rounded-xl border border-[var(--border)] p-3">
            <summary className="cursor-pointer text-sm">{t('Reading document details')}</summary>
            <p className="my-3 break-all text-xs">
              {item.kb} / {item.path}
              <br />
              SHA-256: {item.original_hash}
            </p>
            <form
              className="flex flex-wrap gap-2"
              onSubmit={e => {
                e.preventDefault()
                void perform(async () => {
                  const result = await request<ReadingItem>(
                    '',
                    { title, expected_revision: item.revision },
                    'PATCH'
                  )
                  setItem(result)
                  readingChanged()
                  setNotice(t('Saved'))
                })
              }}
            >
              <input
                aria-label={t('Reading title')}
                className={`${field} flex-1`}
                value={title}
                onChange={e => setTitle(e.target.value)}
                required
                maxLength={500}
                disabled={busy}
              />
              <button
                disabled={busy || title === item.title}
                className={pageActionClass('secondary')}
              >
                {t('Rename')}
              </button>
            </form>
            <button
              className={`${pageActionClass('secondary')} mt-3`}
              disabled={busy}
              onClick={() => setConfirmDelete(v => !v)}
            >
              {t('Delete reading copy')}
            </button>
            {confirmDelete && (
              <div className="mt-3 text-sm">
                <p>
                  {t(
                    'Delete this reading copy, position and annotations? Authored notes already clipped from it remain available.'
                  )}
                </p>
                <button
                  className={`${pageActionClass()} mt-2`}
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      await request('', { expected_revision: item.revision }, 'DELETE')
                      updateDraft(null)
                      readingChanged()
                      router.push(scopedUrl('/learning/reading', scope))
                    })
                  }
                >
                  {t('Confirm delete')}
                </button>
              </div>
            )}
          </details>
          <button
            type="button"
            className={`${pageActionClass('secondary')} mb-3 sm:hidden`}
            aria-expanded={showPosition}
            aria-controls="reading-position"
            onClick={() => setShowPosition(v => !v)}
          >
            {t('Reading position: line {{line}}', { line: item.progress.line })}
            {progressDirty ? ' •' : ''}
          </button>
          <form
            id="reading-position"
            className={`mb-5 flex-wrap items-end gap-3 rounded-xl border border-[var(--border)] p-4 ${showPosition ? 'flex' : 'hidden'} sm:flex`}
            onSubmit={e => {
              e.preventDefault()
              void perform(async () => {
                const progress = await request<ReadingProgress>(
                  '/progress',
                  { line, completed, expected_revision: item.progress.revision },
                  'PUT'
                )
                setItem(v => (v ? { ...v, progress } : v))
                readingChanged()
                setNotice(t('Reading position saved'))
              })
            }}
          >
            <label className="text-sm">
              {t('Saved reading line')}
              <input
                aria-label={t('Saved reading line')}
                className={`${field} ml-2 w-24`}
                type="number"
                min={1}
                max={lines.length}
                value={line}
                onChange={e => setLine(Number(e.target.value))}
                disabled={busy}
                required
              />
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={completed}
                onChange={e => setCompleted(e.target.checked)}
                disabled={busy}
              />
              {t('Finished reading')}
            </label>
            <button className={pageActionClass()} disabled={busy || !progressDirty}>
              {t('Save position')}
            </button>
            <button
              type="button"
              className={pageActionClass('secondary')}
              disabled={busy}
              onClick={() => reveal(item.progress.line)}
            >
              {t('Resume saved position')}
            </button>
            <p className="w-full text-xs text-[var(--muted-foreground)]">
              {t(
                progressDirty
                  ? 'Position changes have not been saved.'
                  : 'Choose a line and save your position to resume later.'
              )}
            </p>
          </form>
          <div className="grid min-w-0 gap-5 xl:grid-cols-[minmax(0,1fr)_340px]">
            <section className="min-w-0" aria-label={t('Source text')}>
              <div className="mb-3 flex flex-wrap items-end gap-2">
                <form
                  className="flex min-w-0 flex-wrap items-end gap-2"
                  onSubmit={e => {
                    e.preventDefault()
                    reveal(jump)
                  }}
                >
                  <label className="text-xs">
                    {t('Go to line')}
                    <input
                      aria-label={t('Go to line')}
                      className={`${field} ml-2 w-24`}
                      type="number"
                      min={1}
                      max={lines.length}
                      value={jump}
                      onChange={e => setJump(Number(e.target.value))}
                      required
                    />
                  </label>
                  <button className={pageActionClass('secondary')}>{t('Go')}</button>
                </form>
                <button
                  className={pageActionClass('secondary')}
                  disabled={!!draft || busy}
                  onClick={() => newDraft()}
                >
                  {t('New annotation')}
                </button>
              </div>
              <form
                className="mb-3 flex min-w-0 flex-wrap gap-2"
                onSubmit={e => {
                  e.preventDefault()
                  const term = query.toLocaleLowerCase()
                  let n = lines.findIndex(
                    (v, i) => i > (match ?? -1) && v.toLocaleLowerCase().includes(term)
                  )
                  if (n < 0) n = lines.findIndex(v => v.toLocaleLowerCase().includes(term))
                  if (n >= 0) {
                    setMatch(n)
                    reveal(n + 1)
                  } else {
                    setMatch(null)
                    setNotice(t('No results found'))
                  }
                }}
              >
                <input
                  aria-label={t('Search this document')}
                  placeholder={t('Search this document')}
                  className={`${field} flex-1`}
                  value={query}
                  onChange={e => {
                    setQuery(e.target.value)
                    setMatch(null)
                  }}
                />
                <button className={pageActionClass('secondary')} disabled={!query.trim()}>
                  {t('Find next')}
                </button>
              </form>
              <p className="mb-2 text-xs text-[var(--muted-foreground)]">
                {t(
                  'Select text or choose a line range to annotate. Click a line number to set the position before saving.'
                )}
              </p>
              <div
                ref={root}
                onMouseUp={selectText}
                onTouchEnd={selectText}
                className="max-h-[65vh] overflow-y-auto rounded-xl border border-[var(--border)] p-2 sm:p-4"
              >
                {lines.slice(offset, offset + PAGE).map((value, i) => {
                  const n = offset + i + 1
                  const a = annotations.find(a => a.start_line <= n && a.end_line >= n)
                  return (
                    <div
                      id={`L${n}`}
                      data-reading-line={n}
                      key={n}
                      className={`flex min-w-0 scroll-mt-4 gap-2 rounded border-l-2 py-1 ${draft && draft.start_line <= n && draft.end_line >= n ? colors[draft.color] : a ? colors[a.color] : 'border-transparent'} ${match === n - 1 ? 'ring-1 ring-[var(--ring)]' : ''}`}
                    >
                      <button
                        aria-label={t('Set position to line {{line}}', { line: n })}
                        className="w-10 shrink-0 self-start select-none text-right font-mono text-xs text-[var(--muted-foreground)]"
                        onClick={() => setLine(n)}
                        disabled={busy}
                      >
                        {n}
                      </button>
                      <span className="min-w-0 whitespace-pre-wrap break-words font-mono text-sm leading-6 [overflow-wrap:anywhere]">
                        {value || '\u00a0'}
                      </span>
                    </div>
                  )
                })}
              </div>
              <div className="mt-3 flex flex-wrap items-center gap-3">
                <button
                  className={pageActionClass('secondary')}
                  disabled={offset === 0}
                  onClick={() => {
                    setOffset(Math.max(0, offset - PAGE))
                    root.current?.scrollTo(0, 0)
                  }}
                >
                  {t('Previous')}
                </button>
                <span className="text-xs">
                  {offset + 1}–{Math.min(lines.length, offset + PAGE)} / {lines.length}
                </span>
                <button
                  className={pageActionClass('secondary')}
                  disabled={offset + PAGE >= lines.length}
                  onClick={() => {
                    setOffset(offset + PAGE)
                    root.current?.scrollTo(0, 0)
                  }}
                >
                  {t('Next')}
                </button>
              </div>
            </section>
            <aside className="min-w-0" aria-label={t('Annotations')}>
              <h2 className="mb-3 font-semibold">
                {t('Annotations')} ({annotations.length})
              </h2>
              {draft && (
                <form
                  aria-label={t('Annotation draft')}
                  className="mb-4 space-y-3 rounded-xl border border-[var(--border)] p-3"
                  onSubmit={e => {
                    e.preventDefault()
                    void perform(async () => {
                      const a = draft.id
                        ? await request<ReadingAnnotation>(
                            `/annotations/${draft.id}`,
                            {
                              note: draft.note,
                              color: draft.color,
                              expected_revision: draft.expected_revision,
                            },
                            'PUT'
                          )
                        : await request<ReadingAnnotation>('/annotations', draft)
                      setAnnotations(v =>
                        [...v.filter(prior => prior.id !== a.id), a].sort(
                          (a, b) => a.start_line - b.start_line
                        )
                      )
                      updateDraft(null)
                      setNotice(t('Annotation saved'))
                    })
                  }}
                >
                  <p className="text-xs text-[var(--muted-foreground)]">
                    {t(
                      'Your annotation draft is kept when you reload. Discard it to edit the latest saved version.'
                    )}
                  </p>
                  <div className="flex flex-wrap gap-2">
                    {(['start_line', 'end_line'] as const).map(k => (
                      <label key={k} className="text-xs">
                        {t(k === 'start_line' ? 'Start line' : 'End line')}
                        <input
                          aria-label={t(k === 'start_line' ? 'Start line' : 'End line')}
                          className={`${field} block w-24`}
                          type="number"
                          min={k === 'end_line' ? draft.start_line : 1}
                          max={lines.length}
                          value={draft[k]}
                          required
                          disabled={busy || !!draft.id}
                          onChange={e => updateDraft({ ...draft, [k]: Number(e.target.value) })}
                        />
                      </label>
                    ))}
                  </div>
                  <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap break-words text-xs [overflow-wrap:anywhere]">
                    {selectedQuote}
                  </pre>
                  <label className="block text-xs">
                    {t('Annotation note')}
                    <textarea
                      aria-label={t('Annotation note')}
                      className={`${field} mt-1 block min-h-24 w-full`}
                      value={draft.note}
                      maxLength={4000}
                      onChange={e => updateDraft({ ...draft, note: e.target.value })}
                      disabled={busy}
                    />
                  </label>
                  <label className="block text-xs">
                    {t('Highlight color')}
                    <select
                      aria-label={t('Highlight color')}
                      className={`${field} ml-2`}
                      value={draft.color}
                      onChange={e => updateDraft({ ...draft, color: e.target.value })}
                      disabled={busy}
                    >
                      {Object.keys(colors).map(color => (
                        <option key={color} value={color}>
                          {t(color)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <div className="flex flex-wrap gap-2">
                    <button className={pageActionClass()} disabled={busy}>
                      {t('Save annotation')}
                    </button>
                    <button
                      type="button"
                      className={pageActionClass('secondary')}
                      disabled={busy}
                      onClick={() => updateDraft(null)}
                    >
                      {t('Discard draft')}
                    </button>
                  </div>
                </form>
              )}
              <div className="space-y-3">
                {annotations.map(a => (
                  <article
                    key={a.id}
                    className={`min-w-0 rounded-xl border p-3 ${colors[a.color] || ''}`}
                  >
                    <button className="text-xs underline" onClick={() => reveal(a.start_line)}>
                      {t('Lines {{start}}–{{end}}', { start: a.start_line, end: a.end_line })}
                    </button>
                    <pre className="my-2 max-h-40 overflow-y-auto whitespace-pre-wrap break-words text-xs [overflow-wrap:anywhere]">
                      {a.quote}
                    </pre>
                    <p className="whitespace-pre-wrap break-words text-sm [overflow-wrap:anywhere]">
                      {a.note}
                    </p>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <button
                        className={pageActionClass('secondary')}
                        disabled={busy || !!draft}
                        onClick={() =>
                          updateDraft({
                            command_id: crypto.randomUUID(),
                            source_id: a.source_id,
                            start_line: a.start_line,
                            end_line: a.end_line,
                            note: a.note,
                            color: a.color,
                            id: a.id,
                            expected_revision: a.revision,
                          })
                        }
                      >
                        {t('Edit annotation')}
                      </button>
                      <button
                        className={pageActionClass('secondary')}
                        disabled={busy || !!draft}
                        onClick={() =>
                          void perform(async () => {
                            await request(
                              `/annotations/${a.id}`,
                              { expected_revision: a.revision },
                              'DELETE'
                            )
                            setAnnotations(v => v.filter(prior => prior.id !== a.id))
                            setNotice(t('Annotation deleted'))
                          })
                        }
                      >
                        {t('Delete annotation')}
                      </button>
                      <button
                        className={pageActionClass('secondary')}
                        disabled={busy}
                        onClick={() =>
                          void perform(async () => {
                            const page = await request<{ id: string }>(
                              `/annotations/${a.id}/note`,
                              { expected_revision: a.revision }
                            )
                            setNoteLinks(v => ({ ...v, [a.id]: page.id }))
                            notifyCoWriterChanged()
                            setNotice(t('Passage saved as an editable note'))
                          })
                        }
                      >
                        {t('Save passage as note')}
                      </button>
                      {noteLinks[a.id] && (
                        <Link
                          className="self-center text-sm underline"
                          href={scopedUrl(`/co-writer/${noteLinks[a.id]}`, scope)}
                        >
                          {t('Open note')}
                        </Link>
                      )}
                    </div>
                  </article>
                ))}
              </div>
              {!annotations.length && !draft && (
                <p className="text-sm text-[var(--muted-foreground)]">{t('No annotations yet')}</p>
              )}
            </aside>
          </div>
        </>
      )}
    </FeaturePage>
  )
}
