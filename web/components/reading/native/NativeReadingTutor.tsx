'use client'
import dynamic from 'next/dynamic'
import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { apiFetch, apiUrl } from '@/lib/api'
import { scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { pageActionClass } from '@/components/layout/FeaturePage'
import type { ReadingItem } from '@/lib/native-reading'
const Markdown = dynamic(() => import('@/components/common/MarkdownRenderer'), { ssr: false })
type Message = { id: number; role: string; content: string }
type Turn = { id: string; status: string }
type Pending = { fingerprint: string; id: string }
export default function NativeReadingTutor({
  item,
  scope,
  start,
  end,
}: {
  item: ReadingItem
  scope: string
  start: number
  end: number
}) {
  const { t } = useTranslation()
  const key = `lorevia.reading-tutor:${scope}:${item.id}`
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [messages, setMessages] = useState<Message[]>([])
  const [active, setActive] = useState<Turn | null>(null)
  const [partial, setPartial] = useState('')
  const [question, setQuestion] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [ready, setReady] = useState(false)
  const [syncing, setSyncing] = useState(false)
  const pending = useRef<Pending | null>(null)
  const cancellation = useRef<{ turn: string; id: string } | null>(null)
  const savedRequest = (() => {
    if (!pending.current) return null
    try {
      const body = JSON.parse(pending.current.fingerprint)
      return body.capability === 'reading_tutor' &&
        body.content === question &&
        (body.session_id ?? null) === sessionId &&
        body.reading_target?.item_id === item.id &&
        body.reading_target?.source_id === item.source_id
        ? body
        : null
    } catch {
      return null
    }
  })()
  const request = useCallback(
    async <T,>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> => {
      const r = await apiFetch(
        apiUrl(scopedUrl(path, scope)),
        body === undefined
          ? { cache: 'no-store', signal }
          : {
              method: 'POST',
              signal,
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }
      )
      const value = await r.json()
      if (!r.ok)
        throw Object.assign(new Error(value.detail || `Request failed (${r.status})`), {
          status: r.status,
        })
      return value as T
    },
    [scope]
  )
  useEffect(() => {
    try {
      const saved = JSON.parse(browserStorage.readRaw('session', key) || '{}')
      if (typeof saved.sessionId === 'string') setSessionId(saved.sessionId)
      if (typeof saved.question === 'string') setQuestion(saved.question)
      if (
        saved.pending &&
        typeof saved.pending.id === 'string' &&
        typeof saved.pending.fingerprint === 'string'
      )
        pending.current = saved.pending
    } catch {
      /* A corrupt local draft cannot change a saved conversation. */
    }
    setReady(true)
  }, [key])
  useEffect(() => {
    if (ready)
      browserStorage.writeRaw(
        'session',
        key,
        JSON.stringify({ sessionId, question, pending: pending.current })
      )
  }, [key, question, sessionId, ready])
  useEffect(() => {
    if (!sessionId) {
      setSyncing(false)
      return
    }
    setSyncing(true)
    const controller = new AbortController()
    let polling = false
    let cursor = { turn: '', seq: 0, text: '' }
    const poll = async () => {
      if (polling || controller.signal.aborted) return
      polling = true
      try {
        const state = await request<{ messages: Message[]; active_turns: Turn[] }>(
          `/api/sessions/${encodeURIComponent(sessionId)}`,
          undefined,
          controller.signal
        )
        if (controller.signal.aborted) return
        setSyncing(false)
        setMessages(state.messages.filter(m => ['user', 'assistant'].includes(m.role)))
        const turn = state.active_turns[0] ?? null
        setActive(turn)
        if (turn) {
          if (cursor.turn !== turn.id) cursor = { turn: turn.id, seq: 0, text: '' }
          const events = await request<{
            events: {
              seq: number
              type: string
              content: string
              metadata: { response?: string }
            }[]
          }>(
            `/api/turns/${encodeURIComponent(turn.id)}/events?after_seq=${cursor.seq}`,
            undefined,
            controller.signal
          )
          for (const event of events.events) {
            cursor.seq = Math.max(cursor.seq, event.seq)
            if (event.type === 'content') cursor.text += event.content
            if (event.type === 'result' && event.metadata.response)
              cursor.text = event.metadata.response
          }
          if (!controller.signal.aborted) setPartial(cursor.text)
        } else setPartial('')
      } catch (cause) {
        if (!controller.signal.aborted) {
          setSyncing(false)
          setError(cause instanceof Error ? cause.message : String(cause))
          if ((cause as { status?: number }).status === 404) setActive(null)
        }
      } finally {
        polling = false
      }
    }
    void poll()
    const timer = setInterval(() => void poll(), 1000)
    return () => {
      controller.abort()
      clearInterval(timer)
    }
  }, [sessionId, request])
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError('')
    const body = savedRequest ?? {
      type: 'start_turn',
      protocol_version: '2.0',
      capability: 'reading_tutor',
      content: question,
      ...(sessionId ? { session_id: sessionId } : {}),
      reading_target: {
        item_id: item.id,
        source_id: item.source_id,
        start_line: start,
        end_line: end,
      },
    }
    const fingerprint = JSON.stringify(body)
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, id: crypto.randomUUID() }
    browserStorage.writeRaw(
      'session',
      key,
      JSON.stringify({ sessionId, question, pending: pending.current })
    )
    try {
      const receipt = await request<{ session_id: string; turn_id: string }>('/api/turns', {
        ...body,
        client_submission_id: pending.current.id,
      })
      pending.current = null
      browserStorage.writeRaw(
        'session',
        key,
        JSON.stringify({ sessionId: receipt.session_id, question: '', pending: null })
      )
      setSessionId(receipt.session_id)
      setActive({ id: receipt.turn_id, status: 'running' })
      setQuestion('')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  return (
    <section
      className="mb-5 rounded-xl border border-[var(--border)] p-4"
      aria-label={t('Reading tutor')}
    >
      <header className="mb-3 flex flex-wrap items-center gap-3">
        <h2 className="flex-1 font-semibold">{t('Reading tutor')}</h2>
        {sessionId && (
          <Link
            className="text-sm underline"
            href={scopedUrl(`/chat/${encodeURIComponent(sessionId)}`, scope)}
          >
            {t('Open conversation')}
          </Link>
        )}
        <button
          className={pageActionClass('secondary')}
          disabled={busy || !!active}
          onClick={() => {
            setSessionId(null)
            setMessages([])
            setPartial('')
            pending.current = null
            setError('')
          }}
        >
          {t('New discussion')}
        </button>
      </header>
      <p className="mb-3 text-sm text-[var(--muted-foreground)]">
        {t(
          'The tutor receives only lines {{start}}–{{end}} from this saved snapshot. Select another passage to change the context.',
          {
            start: savedRequest?.reading_target.start_line ?? start,
            end: savedRequest?.reading_target.end_line ?? end,
          }
        )}
      </p>
      {error && (
        <p role="alert" className="mb-3 break-words text-sm text-red-600">
          {error}
        </p>
      )}
      <div className="mb-3 max-h-96 space-y-3 overflow-y-auto" aria-live="polite">
        {messages.map(m => (
          <article key={m.id} className="rounded-lg bg-[var(--surface-secondary)] p-3">
            <strong className="mb-1 block text-xs">
              {t(m.role === 'user' ? 'You' : 'Reading tutor')}
            </strong>
            <Markdown content={m.content} allowHtml={false} enableImages={false} />
          </article>
        ))}
        {partial && (
          <article className="rounded-lg border border-[var(--border)] p-3">
            <Markdown content={partial} allowHtml={false} enableImages={false} />
          </article>
        )}
      </div>
      {active?.status === 'waiting_input' && (
        <p className="mb-3 text-sm">
          {t(
            'The tutor needs a clarification. Open the conversation to answer, or stop this response.'
          )}
        </p>
      )}
      <form onSubmit={event => void submit(event)} className="flex flex-wrap gap-3">
        <label className="min-w-0 flex-1 text-sm">
          {t('Question about this passage')}
          <textarea
            className="mt-1 block min-h-20 w-full rounded-lg border border-[var(--border)] bg-transparent p-2"
            value={question}
            maxLength={16000}
            required
            disabled={busy || !!active}
            onChange={event => setQuestion(event.target.value)}
          />
        </label>
        <button
          className={`${pageActionClass()} self-end`}
          disabled={!ready || syncing || busy || !!active || !question.trim() || end < start}
        >
          {t(busy ? 'Sending…' : 'Ask tutor')}
        </button>
        {active && (
          <button
            type="button"
            className={`${pageActionClass('secondary')} self-end`}
            disabled={busy}
            onClick={() => {
              if (cancellation.current?.turn !== active.id)
                cancellation.current = { turn: active.id, id: crypto.randomUUID() }
              setBusy(true)
              void request(`/api/turns/${encodeURIComponent(active.id)}/cancel`, {
                command_id: cancellation.current.id,
              })
                .catch(cause => setError(String(cause)))
                .finally(() => setBusy(false))
            }}
          >
            {t('Stop response')}
          </button>
        )}
      </form>
    </section>
  )
}
