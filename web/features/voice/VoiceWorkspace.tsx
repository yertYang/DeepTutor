'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Copy, Mic, MicOff, PhoneOff, Radio, Save, Volume2, VolumeX } from 'lucide-react'
import { apiFetch } from '@/lib/api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import { PageHeader, pageActionClass } from '@/components/layout/FeaturePage'
import { VoiceClient, type VoiceStatus } from './voice-client'

type Config = { revision: number; enabled: boolean; endpoint: string; voice: string; prompt: string; has_api_key: boolean }
type State = { config: Config; voices: string[] }
const field = 'w-full min-w-0 rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm'
const panel = 'min-w-0 rounded-xl border border-[var(--border)] bg-[var(--background)] p-4'
const presets = [
  { label: 'English conversation coach', prompt: 'You are a wise and friendly English conversation coach. Have a natural conversation. Ask one question at a time, keep answers concise, and gently help the learner improve their spoken English.' },
  { label: 'Practice interview', prompt: 'You are a friendly job interviewer. Conduct an English practice interview. Ask one short question at a time. Listen carefully and ask useful follow-up questions. Give constructive feedback when the candidate asks for it.' },
  { label: 'Friendly tutor', prompt: 'You are a wise and friendly teacher. Answer questions or provide advice in a clear and engaging way. Help the learner think through problems by asking one question at a time.' },
]
const statusKeys: Record<VoiceStatus, string> = { idle: 'Ready to connect', permission: 'Waiting for microphone permission', connecting: 'Connecting to PersonaPlex', ready: 'Voice conversation active', stopped: 'Voice conversation ended', error: 'Voice connection error' }

export default function VoiceWorkspace() {
  const { t } = useTranslation()
  const [scope] = useState(activeWorkspaceId)
  const [saved, setSaved] = useState<Config | null>(null)
  const [draft, setDraft] = useState<Config | null>(null)
  const [voices, setVoices] = useState<string[]>([])
  const [apiKey, setApiKey] = useState('')
  const [clearKey, setClearKey] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [status, setStatus] = useState<VoiceStatus>('idle')
  const [transcript, setTranscript] = useState('')
  const [micMuted, setMicMuted] = useState(false)
  const [speakerMuted, setSpeakerMuted] = useState(false)
  const [played, setPlayed] = useState(0)
  const [elapsed, setElapsed] = useState(0)
  const client = useRef<VoiceClient | null>(null)
  const live = useRef(true)
  const active = ['permission', 'connecting', 'ready'].includes(status)
  const dirty = JSON.stringify(saved) !== JSON.stringify(draft) || !!apiKey || clearKey
  const request = useCallback(async <T,>(path: string, method = 'GET', body?: unknown): Promise<T> => {
    const response = await apiFetch(scopedUrl(`/api/voice${path}`, scope), { method, cache: 'no-store',
      ...(body === undefined ? {} : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }) })
    const value = await response.json()
    if (!response.ok) throw new Error(value.message || `Request failed (${response.status})`)
    return value
  }, [scope])
  const reload = useCallback(async () => {
    const value = await request<State>('')
    if (live.current) { setSaved(value.config); setDraft(value.config); setVoices(value.voices); setApiKey(''); setClearKey(false) }
  }, [request])
  useEffect(() => {
    live.current = true
    void reload().catch(e => { if (live.current) setError(String(e)) })
    const stop = () => { client.current?.stop(); client.current = null }
    window.addEventListener('deeptutor:before-workspace-switch', stop)
    return () => { live.current = false; stop(); window.removeEventListener('deeptutor:before-workspace-switch', stop) }
  }, [reload])
  useEffect(() => {
    if (status !== 'ready') return
    const start = Date.now()
    const timer = setInterval(() => setElapsed(Math.floor((Date.now() - start) / 1000)), 1000)
    return () => clearInterval(timer)
  }, [status])
  const save = async () => {
    if (!draft) return
    setSaving(true); setError('')
    try {
      const config = await request<Config>('/config', 'PUT', {
        expected_revision: draft.revision, enabled: draft.enabled, endpoint: draft.endpoint.trim(),
        voice: draft.voice, prompt: draft.prompt, clear_api_key: clearKey, ...(apiKey ? { api_key: apiKey } : {}),
      })
      if (live.current) { setSaved(config); setDraft(config); setApiKey(''); setClearKey(false) }
    } catch (e) { if (live.current) setError(e instanceof Error ? e.message : String(e)) }
    finally { if (live.current) setSaving(false) }
  }
  const start = () => {
    if (!saved || active) return
    client.current?.stop(); setError(''); setTranscript(''); setPlayed(0); setElapsed(0); setMicMuted(false); setSpeakerMuted(false)
    const next = new VoiceClient(scope, {
      status: value => { if (live.current && client.current === next) setStatus(value) },
      error: value => { if (live.current && client.current === next) setError(value) },
      text: value => { if (live.current && client.current === next) setTranscript(old => (old + value).slice(-32000)) },
      played: value => { if (live.current && client.current === next) setPlayed(value) },
    })
    client.current = next
    void next.start(saved.revision)
  }
  const time = `${Math.floor(elapsed / 60).toString().padStart(2, '0')}:${(elapsed % 60).toString().padStart(2, '0')}`
  return <section className="h-full min-h-0 overflow-auto p-4 md:p-6">
    <PageHeader icon={Radio} title={t('Voice conversation')} description={t('Practice spoken English with a tutor or interview partner. Speak naturally and interrupt at any time.')} />
    {error && <div role="alert" className="my-3 rounded-lg bg-red-500/10 p-3 text-sm text-red-700 dark:text-red-300">{t(error)}
      {!active && <button className="ml-3 underline" onClick={() => {
        if (dirty && !window.confirm(t('Discard unsaved voice settings?'))) return
        setError(''); void reload().catch(e => setError(String(e)))
      }}>{t('Reload settings')}</button>}
    </div>}
    {!draft ? <p role="status">{t('Loading...')}</p> : <div className="grid gap-5 xl:grid-cols-[minmax(280px,420px)_minmax(0,1fr)]">
      <fieldset disabled={active || saving} className={`${panel} space-y-4`}>
        <legend className="px-1 font-semibold">{t('PersonaPlex connection')}</legend>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={draft.enabled} onChange={e => setDraft({ ...draft, enabled: e.target.checked })} />{t('Enable voice in this workspace')}</label>
        <label className="block space-y-1 text-sm"><span>{t('Voice service address')}</span><input type="url" className={field} value={draft.endpoint} placeholder="ws://127.0.0.1:8998/api/chat" onChange={e => setDraft({ ...draft, endpoint: e.target.value })} /></label>
        <label className="block space-y-1 text-sm"><span>{t('Voice service API key (optional)')}</span><input type="password" autoComplete="off" className={field} value={apiKey} placeholder={draft.has_api_key ? t('Saved key; leave blank to keep it') : ''} onChange={e => setApiKey(e.target.value)} /></label>
        {draft.has_api_key && <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={clearKey} onChange={e => setClearKey(e.target.checked)} />{t('Remove saved voice API key')}</label>}
        <label className="block space-y-1 text-sm"><span>{t('Voice character')}</span><select className={field} value={draft.voice} onChange={e => setDraft({ ...draft, voice: e.target.value })}>{voices.map(voice => <option key={voice} value={voice}>{voice.replace('.pt', '')}</option>)}</select></label>
        <div><p className="mb-2 text-sm">{t('Conversation role')}</p><div className="flex flex-wrap gap-2">{presets.map(preset => <button key={preset.label} type="button" className={pageActionClass(draft.prompt === preset.prompt ? undefined : 'secondary')} onClick={() => setDraft({ ...draft, prompt: preset.prompt })}>{t(preset.label)}</button>)}</div></div>
        <label className="block space-y-1 text-sm"><span>{t('Role instructions')}</span><textarea className={field} rows={5} maxLength={4000} value={draft.prompt} onChange={e => setDraft({ ...draft, prompt: e.target.value })} /></label>
        <button className={pageActionClass()} disabled={!dirty || saving} onClick={() => void save()}><Save size={15} />{t('Save voice settings')}</button>
        <p className="text-xs leading-5 text-[var(--muted-foreground)]">{t('Connect a separately deployed PersonaPlex service. The official model supports English. Remote services require a secure wss:// address.')}</p>
      </fieldset>
      <div className="min-w-0 space-y-5">
        <section className={`${panel} flex flex-col items-center gap-4 py-8`} aria-label={t('Voice call controls')}>
          <div className={`flex size-24 items-center justify-center rounded-full ${status === 'ready' ? 'animate-pulse bg-emerald-500/15 text-emerald-600' : 'bg-[var(--muted)] text-[var(--muted-foreground)]'}`}><Mic size={38} /></div>
          <p role="status" className="text-center font-medium">{t(statusKeys[status])}</p>
          <p className="font-mono text-xl tabular-nums">{time}</p>
          <div className="flex flex-wrap justify-center gap-2">
            {!active ? <button className={pageActionClass()} disabled={!saved?.enabled || dirty || saving} onClick={start}><Mic size={16} />{t('Start voice conversation')}</button> : <button className={pageActionClass('secondary')} onClick={() => client.current?.stop()}><PhoneOff size={16} />{t('End voice conversation')}</button>}
            <button className={pageActionClass('secondary')} disabled={status !== 'ready'} aria-pressed={micMuted} onClick={() => { client.current?.muteMicrophone(!micMuted); setMicMuted(!micMuted) }}>{micMuted ? <MicOff size={16} /> : <Mic size={16} />}{t(micMuted ? 'Unmute microphone' : 'Mute microphone')}</button>
            <button className={pageActionClass('secondary')} disabled={status !== 'ready'} aria-pressed={speakerMuted} onClick={() => { client.current?.muteSpeaker(!speakerMuted); setSpeakerMuted(!speakerMuted) }}>{speakerMuted ? <VolumeX size={16} /> : <Volume2 size={16} />}{t(speakerMuted ? 'Play voice replies' : 'Mute voice replies')}</button>
          </div>
          {dirty && <p className="text-center text-sm text-[var(--muted-foreground)]">{t('Save your changes before starting a conversation.')}</p>}
          <p className="max-w-lg text-center text-xs leading-5 text-[var(--muted-foreground)]">{t('Your microphone is sent to the configured service while connected. This application does not store the audio. Closing this page ends the call.')}</p>
        </section>
        <section className={panel} aria-label={t('Voice reply captions')}>
          <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="font-semibold">{t('Voice reply captions')}</h2><button className={pageActionClass('secondary')} disabled={!transcript} onClick={() => void navigator.clipboard.writeText(transcript).catch(() => setError(t('Could not copy captions.')))}><Copy size={14} />{t('Copy captions')}</button></div>
          <p className="mt-2 text-xs text-[var(--muted-foreground)]">{t('Captions show the AI’s spoken replies. You can copy them before leaving this page.')}</p>
          <p className="mt-4 max-h-80 overflow-auto whitespace-pre-wrap break-words text-sm leading-7">{transcript || t('Reply captions appear here during the conversation.')}</p>
          {played > 0 && <p className="mt-3 text-xs text-[var(--muted-foreground)]">{t('Audio played')}: {Math.floor(played)} {t('seconds')}</p>}
        </section>
      </div>
    </div>}
  </section>
}
