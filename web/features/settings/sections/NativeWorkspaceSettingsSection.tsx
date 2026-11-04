'use client'
import { useCallback, useEffect, useState } from 'react'
import Link from 'next/link'
import { useTranslation } from 'react-i18next'
import { SettingsPageHeader } from '@/components/settings/shared'
import { pageActionClass } from '@/components/layout/FeaturePage'
import {
  getWorkspaceCatalog,
  saveWorkspace,
  workspaceChatHref,
  type ChatWorkspaceRegistration,
  type WorkspaceCatalog,
} from '@/lib/workspaces-api'

function Row({
  row,
  busy,
  save,
}: {
  row: ChatWorkspaceRegistration
  busy: boolean
  save: (
    row: ChatWorkspaceRegistration,
    input: { name?: string; instructions?: string; archived?: boolean }
  ) => Promise<boolean>
}) {
  const { t } = useTranslation()
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(row.display_name)
  const [instructions, setInstructions] = useState(row.instructions ?? '')
  return (
    <article className="border-t border-[var(--border)] py-5">
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="min-w-0 flex-1 break-words font-medium">
          {row.kind === 'general' ? t('General workspace') : row.display_name}
        </h2>
        {row.archived && (
          <span className="text-xs text-[var(--muted-foreground)]">{t('Archived')}</span>
        )}
        {!row.archived && row.status === 'ready' && (
          <Link className={pageActionClass('secondary')} href={workspaceChatHref(row.workspace_id)}>
            {t('New chat')}
          </Link>
        )}
        <button
          className={pageActionClass('secondary')}
          disabled={busy}
          onClick={() => {
            setEditing(value => !value)
            setName(row.display_name)
            setInstructions(row.instructions ?? '')
          }}
        >
          {t('Edit')}
        </button>
        {row.kind === 'workspace' && (
          <button
            className={pageActionClass('secondary')}
            disabled={busy}
            onClick={() => void save(row, { archived: !row.archived })}
          >
            {row.archived ? t('Restore workspace') : t('Archive workspace')}
          </button>
        )}
      </div>
      <p className="mt-2 break-all font-mono text-xs text-[var(--muted-foreground)]">{row.path}</p>
      {row.error && (
        <p role="alert" className="mt-2 text-sm text-red-600">
          {row.error}
        </p>
      )}
      {editing && (
        <form
          className="mt-4 space-y-3"
          onSubmit={event => {
            event.preventDefault()
            void save(row, { ...(row.kind === 'workspace' ? { name } : {}), instructions }).then(
              ok => {
                if (ok) setEditing(false)
              }
            )
          }}
        >
          {row.kind === 'workspace' && (
            <label className="block text-sm">
              {t('Workspace name')}
              <input
                required
                maxLength={100}
                value={name}
                onChange={event => setName(event.target.value)}
                className="mt-1 block w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
              />
            </label>
          )}
          <label className="block text-sm">
            {t('Workspace instructions')}
            <textarea
              maxLength={16000}
              rows={4}
              value={instructions}
              onChange={event => setInstructions(event.target.value)}
              className="mt-1 block w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
            />
          </label>
          <button disabled={busy || !name.trim()} className={pageActionClass()}>
            {t('Save')}
          </button>
        </form>
      )}
    </article>
  )
}
export default function NativeWorkspaceSettingsSection() {
  const { t } = useTranslation()
  const [catalog, setCatalog] = useState<WorkspaceCatalog | null>(null)
  const [name, setName] = useState('')
  const [instructions, setInstructions] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const refresh = useCallback(async () => {
    setCatalog(await getWorkspaceCatalog())
  }, [])
  useEffect(() => {
    void refresh().catch(cause => setError(cause.message))
  }, [refresh])
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    setError('')
    setNotice('')
    try {
      await action()
      await refresh()
      setNotice(t('Workspace updated.'))
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    } finally {
      setBusy(false)
    }
  }
  return (
    <div>
      <SettingsPageHeader
        title={t('Workspaces')}
        description={t(
          'Keep conversations, learning materials and progress together in one workspace.'
        )}
      />
      {error && (
        <p role="alert" className="mb-4 text-sm text-red-600">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="mb-4 text-sm text-[var(--muted-foreground)]">
          {notice}
        </p>
      )}
      <form
        className="mb-5 space-y-3 rounded-xl border border-[var(--border)] p-4"
        onSubmit={event => {
          event.preventDefault()
          void run(() => saveWorkspace({ name, instructions })).then(ok => {
            if (ok) {
              setName('')
              setInstructions('')
            }
          })
        }}
      >
        <label className="block text-sm">
          {t('Workspace name')}
          <input
            required
            maxLength={100}
            value={name}
            onChange={event => setName(event.target.value)}
            className="mt-1 block w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
          />
        </label>
        <label className="block text-sm">
          {t('Workspace instructions')}
          <textarea
            rows={3}
            maxLength={16000}
            value={instructions}
            onChange={event => setInstructions(event.target.value)}
            className="mt-1 block w-full rounded-lg border border-[var(--border)] bg-[var(--background)] p-2"
          />
        </label>
        <button className={pageActionClass()} disabled={busy || !catalog || !name.trim()}>
          {t('Create workspace')}
        </button>
      </form>
      {catalog?.workspaces.map(row => (
        <Row
          key={row.workspace_id || 'general'}
          row={row}
          busy={busy}
          save={(entry, input) =>
            run(() =>
              saveWorkspace(
                { ...input, expected_revision: entry.revision ?? 0 },
                entry.workspace_id || '__general__'
              )
            )
          }
        />
      ))}
    </div>
  )
}
