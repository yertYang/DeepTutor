'use client'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import Link from 'next/link'
import './note-library.css'
import { FileDown, Layers, X } from 'lucide-react'
import { exportNoteFlashcards, noteRequest } from '@/lib/notes-api'
import { activeWorkspaceId, scopedUrl } from '@/lib/workspace-scope'
import PickerShell from '@/components/common/PickerShell'
interface Card {
  page_id: string
  page_title: string
  block_id: string
  revision: number
  card_id: string
  front: string
  back: string
}
export default function NoteReview() {
  const { t } = useTranslation()
  const [scope] = useState(activeWorkspaceId)
  const command = useRef<{ fingerprint: string; id: string } | null>(null)
  const [cards, setCards] = useState<Card[]>([])
  const [open, setOpen] = useState(false)
  const [revealed, setRevealed] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    void noteRequest<{ cards: Card[] }>('/review/due', undefined, 'GET', scope)
      .then(r => setCards(r.cards))
      .catch(e => setError(String(e)))
  }, [])
  const card = cards[0]
  const exportAll = async () => {
    setBusy(true)
    try {
      const blob = await exportNoteFlashcards(undefined, scope)
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = 'lorevia-flashcards.txt'
      link.click()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
    } catch (err) {
      setError(String(err))
    } finally {
      setBusy(false)
    }
  }
  const grade = async (remembered: boolean) => {
    setBusy(true)
    try {
      const input = { card_id: card.card_id, remembered, expected_revision: card.revision }
      const fingerprint = JSON.stringify([card.page_id, input])
      if (command.current?.fingerprint !== fingerprint) command.current = { fingerprint, id: crypto.randomUUID() }
      await noteRequest(`/${card.page_id}/review`, { ...input, command_id: command.current.id }, 'POST', scope)
      command.current = null
      setCards((await noteRequest<{ cards: Card[] }>('/review/due', undefined, 'GET', scope)).cards)
      setRevealed(false)
    } catch (err) {
      setError(String(err))
    } finally {
      setBusy(false)
    }
  }
  if (!cards.length && !open) return error ? <p role="alert">{error}</p> : null
  return (
    <>
      <button
        className="dt-notes-review-trigger"
        onClick={() => {
          setRevealed(false)
          setOpen(true)
        }}
      >
        <Layers size={14} strokeWidth={1.7} />
        {t('Review flashcards')}
        <span className="dt-notes-review-count">{cards.length}</span>
      </button>
      <PickerShell open={open} onClose={() => setOpen(false)} ariaLabel={t('Review flashcards')}>
        <div className="dt-notes-review">
          <header>
            <span className="dt-notes-review-heading">
              {t('Review flashcards')}
              <button
                type="button"
                className="dt-notes-review-export"
                disabled={busy}
                onClick={() => void exportAll()}
              >
                <FileDown size={13} strokeWidth={1.7} />
                {t('Export all for Anki')}
              </button>
            </span>
            {card && <span>{t('{{count}} left', { count: cards.length })}</span>}
            <button className="dt-note-icon-button" aria-label={t('Close')} onClick={() => setOpen(false)}>
              <X size={15} />
            </button>
          </header>
          {card ? (
            <>
              <div className="dt-notes-review-card">
                <p className="is-front">{card.front}</p>
                {revealed && <p className="is-back">{card.back}</p>}
              </div>
              <Link href={scopedUrl(`/co-writer/${card.page_id}#${card.block_id}`, scope)} className="dt-notes-review-source">
                {t('From “{{page}}”', { page: card.page_title || t('Untitled') })} ↗
              </Link>
              <div className="dt-notes-review-actions">
                {revealed ? (
                  <>
                    <button disabled={busy} onClick={() => void grade(false)}>
                      {t('Again')}
                    </button>
                    <button disabled={busy} className="is-primary" onClick={() => void grade(true)}>
                      {t('Remembered')}
                    </button>
                  </>
                ) : (
                  <button className="is-primary" onClick={() => setRevealed(true)}>
                    {t('Show answer')}
                  </button>
                )}
              </div>
            </>
          ) : (
            <div className="dt-notes-review-done">
              <span aria-hidden>🎉</span>
              <p>{t('All caught up')}</p>
            </div>
          )}
          {error && (
            <p role="alert" className="dt-notes-review-error">
              {error}
            </p>
          )}
        </div>
      </PickerShell>
    </>
  )
}
