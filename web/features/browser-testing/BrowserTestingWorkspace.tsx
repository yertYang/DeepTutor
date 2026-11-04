'use client'

import Link from 'next/link'
import Image from 'next/image'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Download, Globe, Play, Plus, Save, Square, Trash2 } from 'lucide-react'
import { apiFetch, apiUrl } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
import { PageHeader, pageActionClass } from '@/components/layout/FeaturePage'

type Target = { by: string; value: string; name?: string }
type Step = { action: string; url?: string; target?: Target; value?: string; checked?: boolean; count?: number }
type TestCase = { id: string; title: string; steps: Step[]; revision: number }
type Artifact = { id: string; name: string; mime: string }
type Run = { id: string; case_id?: string; case_revision?: number; status: string; source?: string; artifacts: Artifact[]; report?: {
  status: string; error?: string; title?: string; url?: string; duration_ms?: number;
  steps?: { index: number; action: string; status: string; error?: string; duration_ms: number }[];
  snapshot?: { text: string; elements: { tag: string; role?: string; name: string; test_id?: string; placeholder?: string }[] };
  console_errors?: string[];
} }
type Config = { base_url: string; allowed_origins: string[]; enabled: boolean; revision: number }
type State = { config: Config; cases: TestCase[]; runs: Run[]; engine_available: boolean }
const labels: Record<string, string> = {
  navigate: 'Navigate', click: 'Click', fill: 'Fill', press: 'Press key', select: 'Select option', check: 'Check or uncheck',
  expect_visible: 'Expect visible', expect_text: 'Expect text', expect_url: 'Expect URL', expect_title: 'Expect title', expect_count: 'Expect count',
}
const targeted = new Set(['click', 'fill', 'press', 'select', 'check', 'expect_visible', 'expect_text', 'expect_count'])
const valued = new Set(['fill', 'press', 'select', 'expect_text', 'expect_url', 'expect_title'])
const fieldClass = 'w-full min-w-0 rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm'
const panelClass = 'rounded-xl border border-[var(--border)] bg-[var(--background)] p-4'
function freshStep(action: string): Step {
  const step: Step = { action }
  if (action === 'navigate') step.url = '/'
  if (targeted.has(action)) step.target = { by: 'role', value: 'button', name: '' }
  if (valued.has(action)) step.value = ''
  if (action === 'check') step.checked = true
  if (action === 'expect_count') step.count = 1
  return step
}
const emptyCase = (): TestCase => ({ id: '', title: '', revision: 0, steps: [{ action: 'navigate', url: '/' }] })

export default function BrowserTestingWorkspace() {
  const { t } = useTranslation()
  const [scope] = useState(activeWorkspaceId)
  const [state, setState] = useState<State | null>(null)
  const [config, setConfig] = useState<Config | null>(null)
  const [origins, setOrigins] = useState('')
  const [draft, setDraft] = useState<TestCase>(emptyCase)
  const [dirty, setDirty] = useState(false)
  const [runId, setRunId] = useState(() => typeof window === 'undefined' ? '' : new URLSearchParams(window.location.search).get('run') ?? '')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [shot, setShot] = useState('')
  const [jsonMode, setJsonMode] = useState(false)
  const [jsonDraft, setJsonDraft] = useState('')
  const draftKey = `deeptutor.browser-tests:${scope}`
  const request = useCallback(async <T,>(path: string, method = 'GET', body?: unknown): Promise<T> => {
    const response = await apiFetch(scopedUrl(`/api/browser-testing${path}`, scope), {
      method, cache: 'no-store', ...(body === undefined ? {} : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
    })
    const value = await response.json()
    if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
    return value
  }, [scope])
  const refresh = useCallback(async () => { const value = await request<State>(''); setState(value); return value }, [request])
  useEffect(() => {
    let live = true
    void refresh().then(value => {
      if (!live) return
      setConfig(value.config); setOrigins(value.config.allowed_origins.join('\n'))
      const raw = browserStorage.readRaw('local', draftKey)
      if (raw) {
        try { const saved = JSON.parse(raw) as TestCase & { json_draft?: string }; if (Array.isArray(saved.steps) && typeof saved.title === 'string' && typeof saved.id === 'string' && typeof saved.revision === 'number' && saved.steps.every(step => step && typeof step.action === 'string' && step.action in labels)) { setDraft({id: saved.id, title: saved.title, revision: saved.revision, steps: saved.steps}); setDirty(true); if (typeof saved.json_draft === 'string') { setJsonDraft(saved.json_draft); setJsonMode(true) } } } catch { /* Keep server state intact. */ }
      }
    }).catch(e => { if (live) setError(String(e)) })
    return () => { live = false }
  }, [refresh, draftKey])
  const running = state?.runs.some(run => run.status === 'running') ?? false
  useEffect(() => {
    if (!running) return
    const timer = setInterval(() => void refresh().catch(e => setError(String(e))), 1000)
    return () => clearInterval(timer)
  }, [running, refresh])
  const run = state?.runs.find(row => row.id === runId)
  const hasExpectation = draft.steps.some(step => step.action.startsWith('expect_'))
  const screenshot = run?.artifacts.find(file => file.mime === 'image/png')
  useEffect(() => {
    let live = true, objectUrl = ''
    setShot('')
    if (screenshot) void apiFetch(scopedUrl(`/api/browser-testing/artifacts/${encodeURIComponent(screenshot.id)}`, scope))
      .then(async response => { if (!response.ok) throw new Error(t('Could not load screenshot')); return response.blob() })
      .then(blob => { if (live) { objectUrl = URL.createObjectURL(blob); setShot(objectUrl) } })
      .catch(e => { if (live) setError(String(e)) })
    return () => { live = false; if (objectUrl) URL.revokeObjectURL(objectUrl) }
  }, [screenshot?.id, scope, t]) // eslint-disable-line react-hooks/exhaustive-deps
  const change = (next: TestCase) => {
    setDraft(next); setDirty(true)
    browserStorage.writeRaw('local', draftKey, JSON.stringify(next))
  }
  const select = (next: TestCase) => {
    if (dirty && !window.confirm(t('Discard unsaved test changes?'))) return
    setDraft(next); setDirty(false); setJsonMode(false); browserStorage.removeRaw('local', draftKey)
  }
  const perform = async (action: () => Promise<void>) => {
    setBusy(true); setError('')
    try { await action() } catch (e) { setError(e instanceof Error ? e.message : String(e)) }
    finally { setBusy(false) }
  }
  const save = async () => {
    const saved = await request<TestCase>(draft.id ? `/cases/${encodeURIComponent(draft.id)}` : '/cases', draft.id ? 'PUT' : 'POST', {
      title: draft.title, steps: jsonMode ? JSON.parse(jsonDraft) : draft.steps, expected_revision: draft.revision,
    })
    setDraft(saved); setDirty(false); setJsonMode(false); browserStorage.removeRaw('local', draftKey); await refresh()
  }
  const start = async (inspect: boolean) => {
    const result = await request<Run>('/runs', 'POST', { command_id: crypto.randomUUID(),
      ...(inspect ? { url: config?.base_url } : { case_id: draft.id, expected_revision: draft.revision }) })
    setRunId(result.id); await refresh()
  }
  const stepChange = (index: number, step: Step) => change({ ...draft, steps: draft.steps.map((old, i) => i === index ? step : old) })
  const download = async (path: string, name: string) => {
    const response = await apiFetch(scopedUrl(`/api/browser-testing${path}`, scope))
    if (!response.ok) { const value = await response.json(); throw new Error(value.detail || t('Export failed')) }
    const url = URL.createObjectURL(await response.blob()), a = document.createElement('a')
    a.href = url; a.download = name; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000)
  }
  return <section className="h-full min-h-0 overflow-auto p-4 md:p-6">
    <PageHeader icon={Globe} title={t('Browser testing')} description={t('Inspect websites, write browser tests and keep the results in this workspace.')} action={
      <div className="flex gap-2"><button className={pageActionClass('secondary')} disabled={busy} onClick={() => void perform(async () => { await refresh() })}>{t('Refresh')}</button><Link className={pageActionClass('secondary')} href={scopedUrl('/chat?capability=test_cases', scope)}>{t('Write Test Cases')}</Link></div>
    } />
    {error && <div role="alert" className="my-3 rounded-lg bg-red-500/10 p-3 text-sm text-red-700 dark:text-red-300">{error}<button className="ml-3 underline" onClick={() => void perform(async () => { const next = await refresh(); setConfig(next.config); setOrigins(next.config.allowed_origins.join('\n')) })}>{t('Reload page')}</button></div>}
    {!state || !config ? <p role="status">{t('Loading...')}</p> : <div className="space-y-5">
      <section className={panelClass} aria-label={t('Test websites')}>
        <div className="grid gap-4 md:grid-cols-2">
          <label className="space-y-1 text-sm"><span>{t('Base URL')}</span><input className={fieldClass} value={config.base_url} onChange={e => setConfig({ ...config, base_url: e.target.value })} /></label>
          <label className="space-y-1 text-sm"><span>{t('Allowed website origins')}</span><textarea aria-label={t('Allowed website origins')} className={fieldClass} rows={2} value={origins} onChange={e => setOrigins(e.target.value)} /></label>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={config.enabled} onChange={e => setConfig({ ...config, enabled: e.target.checked })} />{t('Allow AI browser tools in this workspace')}</label>
          <button className={pageActionClass()} disabled={busy} onClick={() => void perform(async () => {
            const saved = await request<Config>('/config', 'PUT', { base_url: config.base_url, enabled: config.enabled, expected_revision: config.revision, allowed_origins: origins.split(/[\n,]+/).map(s => s.trim()).filter(Boolean) })
            setConfig(saved); setOrigins(saved.allowed_origins.join('\n')); await refresh()
          })}><Save size={14} />{t('Save websites')}</button>
          <button className={pageActionClass('secondary')} disabled={busy || config.revision === 0} onClick={() => void perform(() => start(true))}>{t('Inspect page')}</button>
        </div>
        <p className="mt-3 text-xs text-[var(--muted-foreground)]">{t('Only the listed websites can be accessed. Each run uses a fresh browser without your personal login sessions.')}</p>
        {config.enabled && <p className="mt-2 text-sm">{t('Choose Write Test Cases mode to inspect the configured website, save a test, run it and fix any failures.')}</p>}
        {!state.engine_available && <p role="status" className="mt-2 text-sm">{t('The browser engine is not installed in this runtime.')}</p>}
      </section>
      <div className="grid gap-5 xl:grid-cols-[220px_minmax(0,1fr)]">
        <aside className={panelClass}>
          <button className={pageActionClass('secondary')} disabled={busy} onClick={() => select(emptyCase())}><Plus size={14} />{t('New browser test')}</button>
          <nav aria-label={t('Saved browser tests')} className="mt-3 space-y-1">{state.cases.map(row => <button key={row.id} disabled={busy} aria-current={draft.id === row.id ? 'page' : undefined}
            className={`w-full truncate rounded-lg px-3 py-2 text-left text-sm hover:bg-[var(--muted)] ${draft.id === row.id ? 'bg-[var(--muted)] font-semibold' : ''}`} onClick={() => select(row)}>{row.title}</button>)}</nav>
        </aside>
        <fieldset disabled={busy} className={`${panelClass} min-w-0`} aria-label={t('Test editor')}>
          {!jsonMode && !hasExpectation && <p className="mb-3 text-sm text-[var(--muted-foreground)]">{t('Add at least one expectation to verify behavior.')}</p>}
          <label className="block space-y-1 text-sm"><span>{t('Test title')}</span><input className={fieldClass} value={draft.title} onChange={e => change({ ...draft, title: e.target.value })} /></label>
          <div className="my-3 flex flex-wrap items-center gap-2">
            <button className={pageActionClass()} disabled={busy || !draft.title.trim() || (!jsonMode && !hasExpectation)} onClick={() => void perform(save)}><Save size={14} />{t('Save test')}</button>
            <button className={pageActionClass('secondary')} disabled={busy || !draft.id} onClick={() => void perform(async () => select(await request<TestCase>(`/cases/${encodeURIComponent(draft.id)}`)))}>{t('Reload test')}</button>
            <button className={pageActionClass('secondary')} disabled={busy || !draft.id || dirty || config.revision === 0} onClick={() => void perform(() => start(false))}><Play size={14} />{t('Run test')}</button>
            <button className={pageActionClass('secondary')} disabled={busy || !draft.id || dirty} onClick={() => void perform(() => download(`/cases/${encodeURIComponent(draft.id)}/export`, `${draft.id}.spec.mjs`))}><Download size={14} />{t('Export Playwright')}</button>
            <button className={pageActionClass('secondary')} disabled={busy} onClick={() => {
              if (jsonMode) { try { const steps = JSON.parse(jsonDraft) as Step[]; if (!Array.isArray(steps) || steps.length > 40 || steps.some(step => !step || typeof step.action !== 'string' || !(step.action in labels))) throw new Error(t('Invalid test steps')); change({ ...draft, steps }); setJsonMode(false) } catch (e) { setError(String(e)) } }
              else { setJsonDraft(JSON.stringify(draft.steps, null, 2)); setJsonMode(true) }
            }}>{t(jsonMode ? 'Step builder' : 'Edit JSON')}</button>
            {draft.id && <button className={pageActionClass('secondary')} disabled={busy} onClick={() => {
              if (window.confirm(t('Delete this browser test? Reports will be kept.'))) void perform(async () => {
                await request(`/cases/${encodeURIComponent(draft.id)}`, 'DELETE', { expected_revision: draft.revision });
                setDraft(emptyCase()); setDirty(false); browserStorage.removeRaw('local', draftKey); await refresh()
              })
            }}><Trash2 size={14} />{t('Delete')}</button>}
            <span role="status" className="ml-auto text-xs text-[var(--muted-foreground)]">{t(dirty ? 'Unsaved changes' : draft.id ? 'Saved' : 'New browser test')}</span>
          </div>
          {jsonMode ? <textarea aria-label={t('Test steps JSON')} className={`${fieldClass} min-h-80 font-mono`} value={jsonDraft} onChange={e => { setJsonDraft(e.target.value); setDirty(true); browserStorage.writeRaw('local', draftKey, JSON.stringify({ ...draft, json_draft: e.target.value })) }} /> : <div className="space-y-3">{draft.steps.map((step, index) => <div key={index} className="rounded-lg bg-[var(--muted)] p-3">
            <div className="mb-2 flex items-center gap-2"><span className="text-xs text-[var(--muted-foreground)]">{index + 1}</span>
              <select aria-label={t('Step action')} className={fieldClass} value={step.action} onChange={e => stepChange(index, freshStep(e.target.value))}>{Object.entries(labels).map(([value, label]) => <option key={value} value={value}>{t(label)}</option>)}</select>
              <button aria-label={t('Move step up')} className="px-2" disabled={index === 0} onClick={() => { const steps = [...draft.steps]; [steps[index - 1], steps[index]] = [steps[index], steps[index - 1]]; change({ ...draft, steps }) }}>↑</button>
              <button aria-label={t('Remove step')} className="px-2" onClick={() => change({ ...draft, steps: draft.steps.filter((_, i) => i !== index) })}><Trash2 size={14} /></button>
            </div>
            <div className="grid gap-2 md:grid-cols-2">
              {step.action === 'navigate' && <label className="text-xs">{t('Page URL')}<input className={fieldClass} value={step.url ?? ''} onChange={e => stepChange(index, { ...step, url: e.target.value })} /></label>}
              {targeted.has(step.action) && <>
                <label className="text-xs">{t('Find element by')}<select className={fieldClass} value={step.target?.by} onChange={e => stepChange(index, { ...step, target: { by: e.target.value, value: '' } })}>
                  {['role', 'label', 'text', 'testid', 'css'].map(by => <option key={by} value={by}>{t({ role: 'Role', label: 'Label', text: 'Text', testid: 'Test ID', css: 'CSS selector' }[by] ?? by)}</option>)}
                </select></label>
                <label className="text-xs">{t('Target value')}<input className={fieldClass} value={step.target?.value ?? ''} onChange={e => stepChange(index, { ...step, target: { ...step.target!, value: e.target.value } })} /></label>
                {step.target?.by === 'role' && <label className="text-xs">{t('Accessible name')}<input className={fieldClass} value={step.target.name ?? ''} onChange={e => stepChange(index, { ...step, target: { ...step.target!, name: e.target.value || undefined } })} /></label>}
              </>}
              {valued.has(step.action) && <label className="text-xs">{t('Value or expected text')}<input className={fieldClass} value={step.value ?? ''} onChange={e => stepChange(index, { ...step, value: e.target.value })} /></label>}
              {step.action === 'check' && <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={step.checked} onChange={e => stepChange(index, { ...step, checked: e.target.checked })} />{t('Checked')}</label>}
              {step.action === 'expect_count' && <label className="text-xs">{t('Expected count')}<input type="number" min={0} max={1000} className={fieldClass} value={step.count ?? 0} onChange={e => stepChange(index, { ...step, count: Number(e.target.value) })} /></label>}
            </div>
          </div>)}<button className={pageActionClass('secondary')} disabled={draft.steps.length >= 40} onClick={() => change({ ...draft, steps: [...draft.steps, freshStep('click')] })}><Plus size={14} />{t('Add step')}</button></div>}
        </fieldset>
      </div>
      <section className={panelClass} aria-label={t('Browser test reports')}>
        <h2 className="mb-3 font-semibold">{t('Browser test reports')}</h2>
        <div className="flex flex-wrap gap-2">{state.runs.map(row => <button key={row.id} className={pageActionClass(row.id === runId ? undefined : 'secondary')} onClick={() => setRunId(row.id)}>
          <span className={`size-2 rounded-full ${row.status === 'passed' ? 'bg-emerald-500' : row.status === 'running' ? 'animate-pulse bg-blue-500' : row.status === 'inspected' ? 'bg-blue-500' : 'bg-red-500'}`} />
          {state.cases.find(c => c.id === row.case_id)?.title ?? t('Page inspection')} · {t(row.status)}
        </button>)}</div>
        {run && <div className="mt-4 space-y-3">
          <div className="flex flex-wrap items-center gap-3"><strong>{t(run.status)}</strong><span className="min-w-0 break-all text-sm text-[var(--muted-foreground)]">{run.report?.url}</span>
            {run.case_revision && <span className="text-xs text-[var(--muted-foreground)]">{t('Test revision')} #{run.case_revision}</span>}
            {run.status === 'running' && <button className={pageActionClass('secondary')} onClick={() => void perform(async () => { await request(`/runs/${encodeURIComponent(run.id)}/cancel`, 'POST', {}); await refresh() })}><Square size={14} />{t('Cancel run')}</button>}
            <button className={pageActionClass('secondary')} onClick={() => void perform(() => download(`/runs/${encodeURIComponent(run.id)}`, 'browser-report.json'))}><Download size={14} />{t('Download report')}</button>
          </div>
          {run.report?.error && <p role="alert" className="text-sm text-red-700 dark:text-red-300">{run.report.error}</p>}
          <ol className="space-y-2">{run.report?.steps?.map(step => <li key={step.index} className="rounded-lg bg-[var(--muted)] p-3 text-sm">
            <span>{step.index + 1}. {t(labels[step.action] ?? step.action)} · {t(step.status)} · {step.duration_ms} {t('ms')}</span>
            {step.error && <pre className="mt-2 whitespace-pre-wrap break-words text-xs text-red-700 dark:text-red-300">{step.error}</pre>}
          </li>)}</ol>
          <div className="grid gap-4 lg:grid-cols-2">
            {shot && <div><Image src={shot} unoptimized width={1280} height={800} alt={t('Browser test screenshot')} className="h-auto w-full rounded-lg border border-[var(--border)]" /></div>}
            {run.report?.snapshot && <details className="min-w-0 rounded-lg bg-[var(--muted)] p-3 text-sm"><summary>{t('Page text and elements')}</summary><pre className="mt-2 max-h-96 overflow-auto whitespace-pre-wrap break-words text-xs">{JSON.stringify(run.report.snapshot, null, 2)}</pre></details>}
          </div>
          {run.artifacts.filter(file => file.mime !== 'image/png').map(file => <a key={file.id} className="inline-block text-sm underline" href={apiUrl(scopedUrl(`/api/browser-testing/artifacts/${encodeURIComponent(file.id)}`, scope))} download>{t('Download trace')}</a>)}
        </div>}
      </section>
    </div>}
  </section>
}
