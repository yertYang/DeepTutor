'use client'

import { useEffect, useState, useCallback } from 'react'
import { apiFetch, apiUrl } from '@/lib/api'

export interface ModuleStatus {
  id: string
  installed: boolean
  enabled: boolean
  ready: boolean
  navigation: string[]
  issues: string[]
  features?: Record<string, boolean>
}

export function moduleRouteAvailable(path: string, modules: ModuleStatus[]): boolean {
  const owners = modules.filter(module => module.navigation.some(
    prefix => path === prefix || path.startsWith(`${prefix}/`)
  ))
  return owners.length === 0 || owners.every(module => module.installed && module.enabled)
}

// One deployment inventory request serves the sidebar, route gate and runtimes.
// A focus/retry refresh observes a server restarted with a different selection.
let pending: Promise<ModuleStatus[]> | null = null
export async function fetchModuleInventory(): Promise<ModuleStatus[]> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 15_000)
  try {
    const response = await apiFetch(apiUrl('/api/modules'), { signal: controller.signal })
    if (!response.ok) throw new Error('Module inventory unavailable')
    const result: unknown = await response.json()
    const modules = result && typeof result === 'object' && 'modules' in result ? result.modules : null
    if (!Array.isArray(modules) || !modules.every(module =>
      module && typeof module === 'object' && typeof module.id === 'string' &&
      typeof module.installed === 'boolean' && typeof module.enabled === 'boolean' &&
      Array.isArray(module.navigation) && module.navigation.every((path: unknown) => typeof path === 'string')
    )) throw new Error('Invalid module inventory')
    return modules as ModuleStatus[]
  } finally {
    clearTimeout(timer)
  }
}

function readModules() {
  if (!pending) {
    pending = fetchModuleInventory()
  }
  return pending
}

export function useModules() {
  const [modules, setModules] = useState<ModuleStatus[] | null>(null)
  const [error, setError] = useState(false)
  const [revision, setRevision] = useState(0)
  const reload = useCallback(() => {
    pending = null
    setError(false)
    setRevision(value => value + 1)
  }, [])
  useEffect(() => {
    let active = true
    void readModules()
      .then(result => { if (active) setModules(result) })
      .catch(() => { if (active) setError(true) })
    window.addEventListener('focus', reload)
    return () => { active = false; window.removeEventListener('focus', reload) }
  }, [revision, reload])
  return { modules, error, reload, available: (path: string) => modules !== null && moduleRouteAvailable(path, modules) }
}
