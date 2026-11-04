import { apiFetch, apiUrl } from '@/lib/api'
import { scopedUrl } from '@/lib/workspace-scope'
import { invalidateClientCache } from '@/lib/client-cache'

export type ReadingProgress = {
  line: number
  completed: boolean
  revision: number
  updated_at: number
}
export type ReadingItem = {
  id: string
  source_id: string
  kind?: 'local' | 'web'
  source_url?: string | null
  mime?: string
  kb: string
  path: string
  original_hash: string
  title: string
  line_count: number
  revision: number
  created_at: number
  updated_at: number
  progress: ReadingProgress
}
export type ReadingAnnotation = {
  id: string
  item_id: string
  source_id: string
  start_line: number
  end_line: number
  quote: string
  note: string
  color: string
  revision: number
  created_at: number
  updated_at: number
}
export type AnnotationDraft = {
  command_id: string
  source_id: string
  start_line: number
  end_line: number
  note: string
  color: string
  id?: string
  expected_revision?: number
}
export function readingUrl(path: string, scope: string) {
  return apiUrl(scopedUrl('/api/reading/items' + path, scope))
}
export async function readingRequest<T>(
  scope: string,
  path = '',
  body?: unknown,
  method = 'POST',
  signal?: AbortSignal
): Promise<T> {
  const response = await apiFetch(
    readingUrl(path, scope),
    body === undefined
      ? { cache: 'no-store', signal }
      : {
          method,
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
          signal,
        }
  )
  const value = await response.json()
  if (!response.ok) throw new Error(value.detail || `Request failed (${response.status})`)
  return value
}
export function readingChanged() {
  invalidateClientCache('navigation:learning-index')
  window.dispatchEvent(new Event('native-reading:changed'))
}
