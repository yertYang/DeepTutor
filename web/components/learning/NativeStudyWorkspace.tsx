'use client'
import dynamic from 'next/dynamic'
import Link from 'next/link'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useRouter, useSearchParams } from 'next/navigation'
import { useTranslation } from 'react-i18next'
import { apiFetch, apiUrl } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { FeaturePage, PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
const Markdown = dynamic(() => import('@/components/common/MarkdownRenderer'), { ssr: false })
type Kind = 'deep_research' | 'deep_question' | 'mastery_path' | 'note_ai' | 'memory_consolidate'
type Source =
  | { kind: 'document'; document_id: string; revision: number; title: string }
  | { kind: 'memory'; layer: string; key: string; revision: number; title: string }
type Question = { id: string; prompt: string; choices: string[] }
type Evidence = { id: string; url: string; title: string; quote: string }
type Unit = { id: string; title: string; goals: string[]; prerequisites: number[] }
type Artifact = {
  id: string
  title: string
  kind: Kind
  phase: string
  revision: number
  objectives: string[]
  evidence: Evidence[]
  applied?: boolean
  proposal_target?: {
    document_id?: string
    layer?: string
    key?: string
    revision: number
    title?: string
  }
  result?: { report?: string; content?: string; questions?: Question[]; units?: Unit[] }
}
type Turn = { id: string; session_id: string; status: string; error: string; capability: Kind }
type Attempt = {
  artifact_id: string
  question_id: string
  answer_index: number
  correct: boolean
  correct_answer_index: number
  explanation: string
}
type Objective = {
  plan_id: string
  objective_id: string
  correct_questions: number
  review_due_at: number
}
type Target = { plan_id: string; objective_id: string; title: string }
type Draft = {
  content?: string
  kind?: Kind
  pending?: { fingerprint: string; id: string }
  kbs?: string[]
  web?: boolean
  target?: Target | null
  source?: Source | null
}
type Request = <T>(path: string, body?: unknown) => Promise<T>
const labels = {
  deep_research: 'Research',
  deep_question: 'Practice questions',
  mastery_path: 'Learning path',
  note_ai: 'AI note editing',
  memory_consolidate: 'Memory consolidation',
}

function PracticeQuestion({
  question,
  record,
  latest,
  request,
  refresh,
}: {
  question: Question
  record: Artifact
  latest?: Attempt
  request: Request
  refresh: () => Promise<void>
}) {
  const { t } = useTranslation()
  const [answer, setAnswer] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const command = useRef<{ answer: number; id: string } | null>(null)
  return (
    <form
      className="my-3 rounded-xl border border-[var(--border)] p-4"
      onSubmit={event => {
        event.preventDefault()
        if (answer === null) return
        if (command.current?.answer !== answer)
          command.current = { answer, id: crypto.randomUUID() }
        setBusy(true)
        setError('')
        void request(`/api/study/${record.id}/attempts`, {
          question_id: question.id,
          answer_index: answer,
          expected_revision: record.revision,
          command_id: command.current.id,
        })
          .then(refresh)
          .catch(cause => setError(cause.message))
          .finally(() => setBusy(false))
      }}
    >
      <p className="mb-3 whitespace-pre-wrap font-medium">{question.prompt}</p>
      {question.choices.map((choice, index) => (
        <label key={index} className="my-2 flex gap-2 text-sm">
          <input
            type="radio"
            name={`${record.id}:${question.id}`}
            checked={answer === index}
            onChange={() => setAnswer(index)}
          />
          <span className="min-w-0 break-words">{choice}</span>
        </label>
      ))}
      <button className={pageActionClass('secondary')} disabled={busy || answer === null}>
        {t('Check answer')}
      </button>
      {latest && (
        <div role="status" className="mt-3 whitespace-pre-wrap text-sm">
          <strong>{latest.correct ? t('Correct') : t('Incorrect')}</strong>
          <p>{question.choices[latest.correct_answer_index]}</p>
          <p>{latest.explanation}</p>
        </div>
      )}
      {error && (
        <p role="alert" className="mt-2 text-sm text-red-600">
          {error}
        </p>
      )}
    </form>
  )
}

export default function NativeStudyWorkspace() {
  const { t } = useTranslation()
  const router = useRouter()
  const params = useSearchParams()
  const [scope] = useState(activeWorkspaceId)
  const key = `deeptutor.native-study:${scope}`
  const initial = useRef<Draft | null>(null)
  if (initial.current === null) {
    try {
      const value = JSON.parse(browserStorage.readRaw('session', key) || '{}')
      initial.current = value && typeof value === 'object' && !Array.isArray(value) ? value : {}
    } catch {
      initial.current = {}
    }
  }
  const [kind, setKind] = useState<Kind>(() => {
    const value = params.get('kind') || initial.current?.kind
    return value && Object.hasOwn(labels, value) ? (value as Kind) : 'deep_research'
  })
  const [content, setContent] = useState(
    (!params.get('kind') || initial.current?.kind === kind
      ? initial.current?.content
      : undefined) ??
      (kind === 'note_ai'
        ? t('Improve clarity while preserving facts and citations.')
        : kind === 'memory_consolidate'
          ? t('Remove repetition while preserving explicit facts and preferences.')
          : '')
  )
  const pending = useRef(initial.current?.pending)
  const formRef = useRef<HTMLFormElement>(null)
  const [kbs, setKbs] = useState<string[]>([])
  const [selectedKbs, setSelectedKbs] = useState<string[]>(
    Array.isArray(initial.current?.kbs)
      ? initial.current.kbs.filter(value => typeof value === 'string').slice(0, 8)
      : []
  )
  const [webAvailable, setWebAvailable] = useState(false)
  const [web, setWeb] = useState(initial.current?.web === true)
  const [target, setTarget] = useState<Target | null>(
    kind === 'deep_question' ? (initial.current?.target ?? null) : null
  )
  const [source, setSource] = useState<Source | null>(() => {
    const revision = Number(params.get('revision'))
    if (params.get('document_id') && Number.isSafeInteger(revision) && revision >= 0)
      return { kind: 'document', document_id: params.get('document_id')!, revision, title: '' }
    if (
      params.get('memory_layer') &&
      params.get('memory_key') &&
      Number.isSafeInteger(revision) &&
      revision >= 0
    )
      return {
        kind: 'memory',
        layer: params.get('memory_layer')!,
        key: params.get('memory_key')!,
        revision,
        title: '',
      }
    return initial.current?.source ?? null
  })
  const [sources, setSources] = useState<Source[]>([])
  const [artifacts, setArtifacts] = useState<Artifact[]>([])
  const [turns, setTurns] = useState<Turn[]>([])
  const [attempts, setAttempts] = useState<Attempt[]>([])
  const [objectives, setObjectives] = useState<Objective[]>([])
  const [selected, setSelected] = useState(params.get('artifact_id') ?? '')
  const linkedArtifact = params.get('artifact_id')
  useEffect(() => {
    if (linkedArtifact) setSelected(linkedArtifact)
  }, [linkedArtifact])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const request = useCallback(
    async <T,>(path: string, body?: unknown): Promise<T> => {
      const response = await apiFetch(
        apiUrl(scopedUrl(path, scope)),
        body === undefined
          ? { cache: 'no-store' }
          : {
              method: 'POST',
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify(body),
            }
      )
      const value = await response.json()
      if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
      return value
    },
    [scope]
  )
  const refresh = useCallback(async () => {
    const [state, progress] = await Promise.all([
      request<{ artifacts: Artifact[]; turns: Turn[] }>('/api/study'),
      request<{ attempts: Attempt[]; objectives: Objective[] }>('/api/study/progress'),
    ])
    setArtifacts(state.artifacts)
    setTurns(state.turns)
    setAttempts(progress.attempts)
    setObjectives(progress.objectives)
  }, [request])
  useEffect(() => {
    let active = true
    const load = () => {
      if (active)
        void refresh().catch(cause => {
          if (active) setError(cause.message)
        })
    }
    load()
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible') load()
    }, 2000)
    void Promise.all([
      request<{ name: string }[]>('/api/knowledge-bases/list'),
      request<{ enabled_optional_tools?: string[] }>('/api/settings/enabled-tools'),
    ])
      .then(([bases, ui]) => {
        if (active) {
          setKbs(bases.map(row => row.name))
          setWebAvailable(Boolean(ui.enabled_optional_tools?.includes('web_search')))
        }
      })
      .catch(cause => {
        if (active) setError(cause.message)
      })
    return () => {
      active = false
      clearInterval(timer)
    }
  }, [refresh, request])
  const loadSources = useCallback(async () => {
    let values: Source[] = []
    if (kind === 'note_ai') {
      const value = await request<{ documents: { id: string; revision: number; title: string }[] }>(
        '/api/documents'
      )
      values = value.documents.map(doc => ({
        kind: 'document',
        document_id: doc.id,
        revision: doc.revision,
        title: doc.title,
      }))
    } else if (kind === 'memory_consolidate') {
      const value = await request<{
        docs: { layer: string; key: string; revision: number; exists: boolean }[]
      }>('/api/memory/overview')
      values = value.docs
        .filter(doc => doc.exists)
        .map(doc => ({
          kind: 'memory',
          layer: doc.layer,
          key: doc.key,
          revision: doc.revision,
          title: doc.layer + '/' + doc.key,
        }))
    }
    setSources(values)
    return values
  }, [kind, request])
  useEffect(() => {
    if (kind === 'note_ai' || kind === 'memory_consolidate')
      void loadSources().catch(cause => setError(cause.message))
  }, [kind, loadSources])
  const sourceId = (value: Source | null) =>
    value ? (value.kind === 'document' ? value.document_id : value.layer + '/' + value.key) : ''
  useEffect(() => {
    browserStorage.writeRaw(
      'session',
      key,
      JSON.stringify({
        content,
        kind,
        pending: pending.current,
        kbs: selectedKbs,
        web,
        target,
        source,
      })
    )
  }, [content, kind, key, selectedKbs, web, target, source])
  const start = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError('')
    const body = {
      type: 'start_turn',
      protocol_version: '2.0',
      capability: kind,
      content,
      knowledge_bases:
        kind === 'note_ai' || kind === 'memory_consolidate' ? [] : [...selectedKbs].sort(),
      tools: web && kind !== 'note_ai' && kind !== 'memory_consolidate' ? ['web_search'] : [],
      study_target:
        kind === 'deep_question' && target
          ? { plan_id: target.plan_id, objective_id: target.objective_id }
          : null,
      document_target:
        kind === 'note_ai' && source?.kind === 'document'
          ? { document_id: source.document_id, expected_revision: source.revision }
          : null,
      memory_target:
        kind === 'memory_consolidate' && source?.kind === 'memory'
          ? { layer: source.layer, key: source.key, expected_revision: source.revision }
          : null,
    }
    const fingerprint = JSON.stringify(body)
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, id: crypto.randomUUID() }
    browserStorage.writeRaw(
      'session',
      key,
      JSON.stringify({
        content,
        kind,
        pending: pending.current,
        kbs: selectedKbs,
        web,
        target,
        source,
      })
    )
    try {
      const receipt = await request<{ turn_id: string }>('/api/turns', {
        ...body,
        client_submission_id: pending.current.id,
      })
      pending.current = undefined
      browserStorage.writeRaw(
        'session',
        key,
        JSON.stringify({ content: '', kind, kbs: selectedKbs, web, target: null, source })
      )
      setContent('')
      setTarget(null)
      setSelected(receipt.turn_id)
      await refresh()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  const record = artifacts.find(row => row.id === selected) ?? artifacts[0]
  const saveNote = async () => {
    if (!record) return
    setBusy(true)
    setError('')
    try {
      const page = await request<{ id: string }>(`/api/study/${record.id}/note`, {
        expected_revision: record.revision,
      })
      router.push(scopedUrl(`/co-writer/${page.id}`, scope))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  const applyProposal = async () => {
    if (
      !record?.proposal_target ||
      !window.confirm(t('Apply the reviewed proposal to its source document?'))
    )
      return
    setBusy(true)
    setError('')
    try {
      await request(`/api/study/${record.id}/apply`, {
        expected_revision: record.revision,
        expected_target_revision: record.proposal_target.revision,
      })
      await refresh()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }
  return (
    <FeaturePage>
      <PageHeader
        title={t('Study workspace')}
        description={t(
          'Research with source citations, practice with feedback, and build a learning path.'
        )}
      />
      {error && (
        <p
          role="alert"
          className="mb-4 rounded-lg border border-red-500/40 p-3 text-sm text-red-600"
        >
          {error}
        </p>
      )}
      <form
        ref={formRef}
        onSubmit={event => void start(event)}
        className="mb-6 space-y-3 rounded-xl border border-[var(--border)] p-4"
      >
        <label className="block text-sm">
          {t('Study mode')}
          <select
            aria-label={t('Study mode')}
            value={kind}
            onChange={event => {
              setKind(event.target.value as Kind)
              setTarget(null)
              setSource(null)
            }}
            className="ml-3 rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
          >
            {Object.entries(labels).map(([id, label]) => (
              <option key={id} value={id}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
        {(kind === 'note_ai' || kind === 'memory_consolidate') && (
          <div className="flex flex-wrap items-center gap-2">
            <label className="text-sm">
              {t('Source document')}
              <select
                aria-label={t('Source document')}
                value={sourceId(source)}
                onChange={event =>
                  setSource(sources.find(value => sourceId(value) === event.target.value) ?? null)
                }
                className="ml-2 max-w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
              >
                <option value="">{t('Select a source')}</option>
                {sources.map(value => (
                  <option key={sourceId(value)} value={sourceId(value)}>
                    {value.title}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="text-sm underline"
              onClick={() =>
                void loadSources()
                  .then(values =>
                    setSource(values.find(value => sourceId(value) === sourceId(source)) ?? null)
                  )
                  .catch(cause => setError(cause.message))
              }
            >
              {t('Reload source')}
            </button>
          </div>
        )}
        {target && (
          <p className="text-sm">
            {t('Selected objective')}: {target.title}{' '}
            <button type="button" className="underline" onClick={() => setTarget(null)}>
              {t('Clear')}
            </button>
          </p>
        )}
        <label className="block text-sm">
          {t('Learning goal')}
          <textarea
            rows={4}
            maxLength={16000}
            required
            value={content}
            onChange={event => setContent(event.target.value)}
            className="mt-1 block w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
          />
        </label>
        {kind !== 'note_ai' && kind !== 'memory_consolidate' && (
          <>
            <fieldset className="flex flex-wrap gap-3">
              <legend className="mb-2 text-sm">{t('Knowledge bases')}</legend>
              {kbs.map(kb => (
                <label key={kb} className="flex gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={selectedKbs.includes(kb)}
                    disabled={!selectedKbs.includes(kb) && selectedKbs.length >= 8}
                    onChange={event =>
                      setSelectedKbs(value =>
                        event.target.checked ? [...value, kb] : value.filter(name => name !== kb)
                      )
                    }
                  />
                  {kb}
                </label>
              ))}
            </fieldset>
            <label className="flex gap-2 text-sm">
              <input
                type="checkbox"
                disabled={!webAvailable}
                checked={web}
                onChange={event => setWeb(event.target.checked)}
              />
              {t('Use configured web search')}
            </label>
          </>
        )}
        {kind === 'deep_research' && !selectedKbs.length && !web && (
          <p className="text-xs text-[var(--muted-foreground)]">
            {t('Select a knowledge base or enable configured web search for research.')}
          </p>
        )}
        <button
          className={pageActionClass()}
          disabled={
            busy ||
            !content.trim() ||
            (kind === 'deep_research' && !selectedKbs.length && !web) ||
            (kind === 'note_ai' && source?.kind !== 'document') ||
            (kind === 'memory_consolidate' && source?.kind !== 'memory')
          }
        >
          {busy ? t('Starting...') : t('Start study')}
        </button>
      </form>
      <section aria-label={t('Study runs')} className="mb-5 space-y-2">
        {turns
          .filter(
            turn => ['queued', 'running', 'waiting_input'].includes(turn.status) || turn.error
          )
          .slice(0, 10)
          .map(turn => (
            <div
              key={turn.id}
              className="flex flex-wrap items-center gap-3 rounded-lg border border-[var(--border)] p-3 text-sm"
            >
              <span>
                {t(labels[turn.capability])} · {t(turn.status)}
              </span>
              {turn.error && <span className="break-words text-red-600">{turn.error}</span>}
              <Link className="underline" href={scopedUrl(`/chat/${turn.session_id}`, scope)}>
                {t('Open conversation')}
              </Link>
              {['queued', 'running', 'waiting_input'].includes(turn.status) && (
                <button
                  className="underline"
                  onClick={() =>
                    void request(`/api/turns/${turn.id}/cancel`, {
                      command_id: `study-cancel-${turn.id}`,
                    })
                      .then(refresh)
                      .catch(cause => setError(cause.message))
                  }
                >
                  {t('Cancel')}
                </button>
              )}
            </div>
          ))}
      </section>
      <div className="grid min-w-0 gap-5 md:grid-cols-[220px_minmax(0,1fr)]">
        <nav aria-label={t('Study artifacts')} className="space-y-2">
          {artifacts.map(row => (
            <button
              key={row.id}
              className={`block w-full break-words rounded-lg p-3 text-left text-sm ${row.id === record?.id ? 'bg-[var(--accent)]' : 'hover:bg-[var(--muted)]'}`}
              onClick={() => setSelected(row.id)}
            >
              {row.title}
              <span className="mt-1 block text-xs text-[var(--muted-foreground)]">
                {t(labels[row.kind])} · {t(row.phase)}
              </span>
            </button>
          ))}
        </nav>
        {record && (
          <article className="min-w-0">
            <div className="mb-3 flex flex-wrap items-center gap-3">
              <h2 className="min-w-0 flex-1 break-words text-lg font-semibold">{record.title}</h2>
              {record.phase === 'published' && (
                <button
                  className={pageActionClass('secondary')}
                  disabled={busy}
                  onClick={() => void saveNote()}
                >
                  {t('Save as note')}
                </button>
              )}
            </div>
            <ul className="mb-4 list-inside list-disc text-sm text-[var(--muted-foreground)]">
              {record.objectives.map((goal, index) => (
                <li key={index}>{goal}</li>
              ))}
            </ul>
            {record.phase === 'published' && record.proposal_target && (
              <div className="mb-4 rounded-lg border border-[var(--border)] p-3">
                <p className="mb-2 text-sm">
                  {t(
                    'Review the proposal before applying it. The source stays unchanged until you apply.'
                  )}
                </p>
                <Link
                  className="mb-3 block break-words text-sm underline"
                  href={scopedUrl(
                    record.proposal_target.document_id
                      ? '/co-writer/' + record.proposal_target.document_id
                      : '/memory/' +
                          record.proposal_target.layer?.toLowerCase() +
                          '/' +
                          record.proposal_target.key,
                    scope
                  )}
                >
                  {t('Source document')}:{' '}
                  {record.proposal_target.title ||
                    record.proposal_target.key ||
                    record.proposal_target.document_id}{' '}
                  · r{record.proposal_target.revision}
                </Link>
                <button
                  className={pageActionClass()}
                  disabled={busy || record.applied}
                  onClick={() => void applyProposal()}
                >
                  {record.applied ? t('Applied') : t('Apply reviewed changes')}
                </button>
              </div>
            )}
            {record.result?.content && (
              <Markdown
                content={record.result.content}
                enableImages={false}
                allowHtml={false}
                enableMermaid={false}
              />
            )}
            {record.result?.report && (
              <Markdown
                content={record.result.report}
                enableImages={false}
                allowHtml={false}
                enableMermaid={false}
              />
            )}
            {record.evidence.map(item => (
              <details
                key={item.id}
                className="my-3 rounded-lg border border-[var(--border)] p-3 text-sm"
              >
                <summary>
                  {item.id} · {item.title}
                </summary>
                <a
                  className="my-2 block break-all underline"
                  href={item.url.startsWith('/') ? apiUrl(item.url) : item.url}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {t('Open source')}
                </a>
                <blockquote className="whitespace-pre-wrap">{item.quote}</blockquote>
              </details>
            ))}
            {record.result?.questions?.map(question => (
              <PracticeQuestion
                key={`${record.id}:${question.id}`}
                question={question}
                record={record}
                latest={attempts
                  .filter(a => a.artifact_id === record.id && a.question_id === question.id)
                  .at(-1)}
                request={request}
                refresh={refresh}
              />
            ))}
            {record.result?.units?.map(unit => {
              const progress = objectives.find(
                o => o.plan_id === record.id && o.objective_id === unit.id
              )
              return (
                <section
                  key={unit.id}
                  className="my-3 rounded-xl border border-[var(--border)] p-4"
                >
                  <h3 className="font-medium">
                    {unit.id} · {unit.title}
                  </h3>
                  <ul className="my-2 list-inside list-disc text-sm">
                    {unit.goals.map((goal, index) => (
                      <li key={index}>{goal}</li>
                    ))}
                  </ul>
                  {unit.prerequisites.length > 0 && (
                    <p className="mb-2 text-xs">
                      {t('Prerequisites')}:{' '}
                      {unit.prerequisites.map(i => record.result?.units?.[i]?.title).join(', ')}
                    </p>
                  )}
                  {progress && (
                    <p className="my-2 text-xs">
                      {t('Correct questions')}: {progress.correct_questions} · {t('Review due')}:{' '}
                      {new Date(progress.review_due_at * 1000).toLocaleDateString()}
                    </p>
                  )}
                  <button
                    className={pageActionClass('secondary')}
                    onClick={() => {
                      setKind('deep_question')
                      setTarget({ plan_id: record.id, objective_id: unit.id, title: unit.title })
                      setContent(`${unit.title}\n${unit.goals.join('\n')}`)
                      formRef.current?.scrollIntoView({ block: 'start', behavior: 'smooth' })
                    }}
                  >
                    {t('Practice this objective')}
                  </button>
                </section>
              )
            })}
          </article>
        )}
      </div>
    </FeaturePage>
  )
}
