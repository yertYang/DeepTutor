'use client'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { apiFetch } from '@/lib/api'
import { scopedUrl } from '@/lib/workspace-scope'
import { browserStorage } from '@/shared/storage'
type Card = { id: string; front: string; back: string; due_at?: number; reviews?: number }
type Deck = { page_id: string; revision: number; cards: Card[] }
export default function NativeFlashcards({ pageId, scope }: { pageId: string; scope: string }) {
  const { t } = useTranslation()
  const [deck, setDeck] = useState<Deck | null>(null)
  const [cards, setCards] = useState<Card[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [current, setCurrent] = useState(0)
  const [revealed, setRevealed] = useState(false)
  const command = useRef<{ fingerprint: string; id: string } | null>(null)
  const key = `lorevia.flashcards:${scope}:${pageId}`
  const request = useCallback(
    async (suffix = '', body?: unknown) => {
      const response = await apiFetch(
        scopedUrl(`/api/documents/${encodeURIComponent(pageId)}/${suffix || 'flashcards'}`, scope),
        body
          ? {
              method: suffix === 'review' ? 'POST' : 'PUT',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            }
          : { cache: 'no-store' }
      )
      const value = await response.json()
      if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
      return value as Deck
    },
    [pageId, scope]
  )
  const accept = (value: Deck) => {
    setDeck(value)
    setCards(value.cards)
    setDirty(false)
    setCurrent(0)
    setRevealed(false)
  }
  useEffect(() => {
    let active = true
    void request()
      .then(value => {
        if (!active) return
        accept(value)
        const raw = browserStorage.readRaw('local', key)
        if (raw)
          try {
            const draft = JSON.parse(raw)
            if (
              Array.isArray(draft.cards) &&
              draft.cards.every(
                (c: Card) =>
                  typeof c.id === 'string' &&
                  typeof c.front === 'string' &&
                  typeof c.back === 'string'
              )
            ) {
              setCards(draft.cards)
              setDirty(true)
              if (draft.revision !== value.revision) {
                setDeck({ ...value, revision: draft.revision })
                setError(
                  t(
                    'Flashcards changed elsewhere. Your local draft is preserved; reload to use the saved cards.'
                  )
                )
              }
            }
          } catch {
            /* Invalid local drafts cannot overwrite the deck. */
          }
      })
      .catch(err => {
        if (active) setError(String(err))
      })
    return () => {
      active = false
    }
  }, [request, key, t])
  const edit = (next: Card[]) => {
    setCards(next)
    setDirty(true)
    browserStorage.writeRaw('local', key, JSON.stringify({ cards: next, revision: deck?.revision }))
  }
  const save = async () => {
    if (!deck) return
    const body = {
      expected_revision: deck.revision,
      cards: cards.map(({ id, front, back }) => ({ id, front, back })),
    }
    const fingerprint = JSON.stringify(body)
    if (command.current?.fingerprint !== fingerprint)
      command.current = { fingerprint, id: crypto.randomUUID() }
    const value = await request('', { ...body, command_id: command.current.id })
    command.current = null
    accept(value)
    browserStorage.removeRaw('local', key)
  }
  const perform = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError('')
    try {
      await fn()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }
  const due = deck?.cards.filter(c => (c.due_at ?? 0) <= Date.now() / 1000) ?? []
  const card = due[current]
  return (
    <section
      className="m-4 rounded-xl border border-[var(--border)] p-4"
      aria-label={t('Flashcards')}
    >
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="flex-1 font-semibold">{t('Flashcards')}</h2>
        <span role="status">{t(dirty ? 'Unsaved changes' : 'Saved')}</span>
        <button
          className="dt-note-button"
          disabled={busy || !deck}
          onClick={() => edit([...cards, { id: crypto.randomUUID(), front: '', back: '' }])}
        >
          {t('Add card')}
        </button>
        <button
          className="dt-note-button"
          disabled={busy || !dirty}
          onClick={() => void perform(save)}
        >
          {t('Save flashcards')}
        </button>
        <button
          className="dt-note-button"
          disabled={busy}
          onClick={() =>
            void perform(async () => {
              accept(await request())
              browserStorage.removeRaw('local', key)
            })
          }
        >
          {t('Reload saved cards')}
        </button>
        <a
          className="dt-note-button"
          href={scopedUrl(`/api/documents/${encodeURIComponent(pageId)}/flashcards/export`, scope)}
          download
        >
          {t('Export flashcards (Anki)')}
        </a>
      </div>
      {error && (
        <p role="alert" className="my-3 text-red-600">
          {error}
        </p>
      )}
      <p className="my-3 text-sm text-[var(--text-secondary)]">
        {t(
          'Edit both sides, save, then reveal the answer before grading. Changed cards restart their review schedule.'
        )}
      </p>
      <div className="max-h-96 overflow-auto">
        {cards.map((c, index) => (
          <div
            key={c.id}
            className="my-3 grid gap-2 rounded-lg border border-[var(--border)] p-3 sm:grid-cols-2"
          >
            <label>
              {t('Front')} {index + 1}
              <textarea
                className="mt-1 min-h-20 w-full rounded border border-[var(--border)] bg-transparent p-2"
                disabled={busy}
                value={c.front}
                onChange={e =>
                  edit(cards.map(v => (v.id === c.id ? { ...v, front: e.target.value } : v)))
                }
              />
            </label>
            <label>
              {t('Back')} {index + 1}
              <textarea
                className="mt-1 min-h-20 w-full rounded border border-[var(--border)] bg-transparent p-2"
                disabled={busy}
                value={c.back}
                onChange={e =>
                  edit(cards.map(v => (v.id === c.id ? { ...v, back: e.target.value } : v)))
                }
              />
            </label>
            <button
              className="dt-note-button justify-self-start"
              disabled={busy}
              onClick={() => edit(cards.filter(v => v.id !== c.id))}
            >
              {t('Remove card')} {index + 1}
            </button>
          </div>
        ))}
      </div>
      {!dirty && card && (
        <div
          className="mt-4 rounded-xl bg-[var(--surface-secondary)] p-4"
          aria-label={t('Review flashcards')}
        >
          <p>{card.front}</p>
          {revealed && <p className="mt-3 whitespace-pre-wrap">{card.back}</p>}
          <div className="mt-3 flex gap-3">
            {!revealed ? (
              <button className="dt-note-button" onClick={() => setRevealed(true)}>
                {t('Show answer')}
              </button>
            ) : (
              [false, true].map(remembered => (
                <button
                  key={String(remembered)}
                  className="dt-note-button"
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      const body = {
                        expected_revision: deck!.revision,
                        card_id: card.id,
                        remembered,
                      }
                      const fingerprint = JSON.stringify(body)
                      if (command.current?.fingerprint !== fingerprint)
                        command.current = { fingerprint, id: crypto.randomUUID() }
                      const value = await request('review', {
                        ...body,
                        command_id: command.current.id,
                      })
                      command.current = null
                      setDeck(value)
                      setCards(value.cards)
                      setCurrent(0)
                      setRevealed(false)
                    })
                  }
                >
                  {t(remembered ? 'Remembered' : 'Again')}
                </button>
              ))
            )}
          </div>
        </div>
      )}
      {!dirty && deck && !card && (
        <p className="mt-4">
          {t('No flashcards due. Come back when the next review is scheduled.')}
        </p>
      )}
    </section>
  )
}
