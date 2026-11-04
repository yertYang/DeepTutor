'use client'

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { BlockNoteSchema, defaultBlockSpecs, type PartialBlock } from '@blocknote/core'
import { BlockNoteViewRaw, useCreateBlockNote } from '@blocknote/react'
import { en, zh } from '@blocknote/core/locales'
import { useTranslation } from 'react-i18next'
import type { NoteBlock, NotePage } from '@/lib/notes-api'
import { subscribeToThemeChanges } from '@/lib/theme'
import '@blocknote/core/style.css'
import './notes.css'

const schema = BlockNoteSchema.create({ blockSpecs: {
  paragraph: defaultBlockSpecs.paragraph, heading: defaultBlockSpecs.heading,
  bulletListItem: defaultBlockSpecs.bulletListItem, numberedListItem: defaultBlockSpecs.numberedListItem,
  checkListItem: defaultBlockSpecs.checkListItem, toggleListItem: defaultBlockSpecs.toggleListItem,
  quote: defaultBlockSpecs.quote, codeBlock: defaultBlockSpecs.codeBlock, table: defaultBlockSpecs.table,
} })

export function nativeBlocksEditable(blocks: NoteBlock[] | null): boolean {
  if (blocks === null) return true
  if (new TextEncoder().encode(JSON.stringify(blocks)).byteLength > 1024 * 1024) return false
  const ids = new Set<string>()
  const inline = (items: unknown, depth = 0): boolean => depth <= 16 && Array.isArray(items) && items.every(item => {
    if (!item || typeof item !== 'object') return false
    if (item.type === 'text') return typeof item.text === 'string' && (!item.styles || typeof item.styles === 'object')
    return item.type === 'link' && typeof item.href === 'string' &&
      /^(https?:\/\/|mailto:|\/(?!\/)|#)/i.test(item.href) && !/[\\\x00-\x1f\x7f]/.test(item.href) && inline(item.content, depth + 1)
  })
  const visit = (items: NoteBlock[], depth = 0): boolean => depth <= 16 && Array.isArray(items) && items.every(block => {
    if (!block || !Object.hasOwn(schema.blockSchema, block.type) || typeof block.id !== 'string' || ids.has(block.id)) return false
    ids.add(block.id)
    if (ids.size > 10000 || (block.props && (typeof block.props !== 'object' ||
      Object.values(block.props).some(value => !['string', 'number', 'boolean'].includes(typeof value))))) return false
    const table = block.type === 'table' ? block.content as { rows?: { cells?: unknown[] }[] } | undefined : null
    const valid = table ? Array.isArray(table.rows) && table.rows.length > 0 && table.rows.every(row =>
      Array.isArray(row.cells) && row.cells.length > 0 && row.cells.every(cell => inline(
        cell && typeof cell === 'object' && 'content' in cell ? cell.content : cell))) : inline(block.content ?? [])
    return valid && visit(block.children ?? [], depth + 1)
  })
  return visit(blocks)
}

export default function NativeNoteEditor({ page, onChange, disabled = false }: {
  page: NotePage
  onChange: (blocks: NoteBlock[]) => void
  disabled?: boolean
}) {
  const { t, i18n } = useTranslation()
  const ready = useRef(false)
  const [error, setError] = useState('')
  const theme = useSyncExternalStore<'light' | 'dark'>(subscribeToThemeChanges,
    () => document.documentElement.classList.contains('dark') ? 'dark' : 'light', () => 'light')
  const editor = useCreateBlockNote({
    schema,
    dictionary: { ...(i18n.language.startsWith('zh') ? zh : en), placeholders: {
      ...(i18n.language.startsWith('zh') ? zh : en).placeholders,
      default: t('Start writing…'),
    } },
    initialContent: page.blocks?.length ? page.blocks as PartialBlock<typeof schema.blockSchema>[] : undefined,
    pasteHandler: ({ defaultPasteHandler }) => {
      return defaultPasteHandler({ prioritizeMarkdownOverHTML: true, plainTextAsMarkdown: true })
    },
  })
  useEffect(() => {
    let active = true
    const initialize = async () => {
      if (!page.blocks && page.content) {
        const blocks = await editor.tryParseMarkdownToBlocks(page.content)
        if (!active) return
        editor.replaceBlocks(editor.document, blocks)
      }
      if (active) ready.current = true
    }
    void initialize().catch(err => setError(err instanceof Error ? err.message : String(err)))
    return () => { active = false; ready.current = false }
    // This editor is mounted once per loaded revision, never on every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor])
  const action = (label: string, run: () => void) => <button type="button" className="dt-note-button" disabled={disabled}
    onMouseDown={event => event.preventDefault()} onClick={run}>{t(label)}</button>
  return <div className="dt-note-editor">
    {error ? <p role="alert">{error}</p> : null}
    <div className="flex flex-wrap gap-1 border-b border-[var(--border)] pb-2 mb-4" role="toolbar" aria-label={t('Formatting')}>
      <select aria-label={t('Block type')} className="dt-note-button" disabled={disabled} defaultValue="paragraph" onChange={event => {
        const kind = event.target.value
        const block = editor.getTextCursorPosition().block
        if (kind.startsWith('heading')) editor.updateBlock(block, { type: 'heading', props: { level: Number(kind.slice(-1)) as 1 | 2 | 3 } })
        else editor.updateBlock(block, { type: kind as 'paragraph' | 'bulletListItem' | 'numberedListItem' | 'checkListItem' | 'quote' | 'codeBlock' })
        editor.focus()
      }}>
        {['paragraph', 'heading1', 'heading2', 'heading3', 'bulletListItem', 'numberedListItem', 'checkListItem', 'quote', 'codeBlock'].map((kind, index) =>
          <option key={kind} value={kind}>{t(['Text', 'Heading 1', 'Heading 2', 'Heading 3', 'Bullet list', 'Numbered list', 'Checklist', 'Quote', 'Code'][index])}</option>)}
      </select>
      {action('Bold', () => editor.toggleStyles({ bold: true }))}
      {action('Italic', () => editor.toggleStyles({ italic: true }))}
      {action('Undo', () => { editor.undo() })}
      {action('Redo', () => { editor.redo() })}
      {action('Table', () => { editor.insertBlocks([{ type: 'table', content: { type: 'tableContent', rows: [
        { cells: [[], []] }, { cells: [[], []] },
      ] } }], editor.getTextCursorPosition().block, 'after') })}
    </div>
    <BlockNoteViewRaw editor={editor} theme={theme} editable={!error && !disabled} formattingToolbar={false} linkToolbar={false}
      slashMenu={false} sideMenu={false} filePanel={false} tableHandles={false} emojiPicker={false} comments={false}
      onChange={() => { if (ready.current) onChange(editor.document as NoteBlock[]) }} />
  </div>
}
