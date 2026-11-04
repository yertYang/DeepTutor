'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import Image from 'next/image'
import { useTranslation } from 'react-i18next'
import { BRAND_ASSETS, PRODUCT_REPOSITORY_URL, PRODUCT_RELEASES_URL } from '@/lib/product'
import { desktopReleases, type DesktopPlatform, type DesktopRelease } from '@/lib/desktop-releases'
import { pageActionClass } from '@/components/layout/FeaturePage'

const platforms: { id: DesktopPlatform; name: string; description: string }[] = [
  { id: 'macos-arm64', name: 'macOS · Apple Silicon', description: 'For Macs with M-series chips' },
  { id: 'macos-x64', name: 'macOS · Intel', description: 'For Macs with Intel processors' },
  { id: 'windows-x64', name: 'Windows · x64', description: 'For 64-bit Windows PCs' },
]
export default function DesktopDownloads() {
  const { t } = useTranslation()
  const [releases, setReleases] = useState<DesktopRelease[] | null>(null)
  const [error, setError] = useState(false)
  const [revision, setRevision] = useState(0)
  const [preview, setPreview] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), 15_000)
    let active = true
    setError(false)
    setReleases(null)
    const repository = new URL(PRODUCT_REPOSITORY_URL).pathname
    void fetch(`https://api.github.com/repos${repository}/releases?per_page=30`, {
      headers: { Accept: 'application/vnd.github+json' },
      signal: controller.signal,
      cache: 'no-store',
      credentials: 'omit',
    })
      .then(async response => {
        if (!response.ok) throw new Error('Desktop release list unavailable')
        const rows = desktopReleases(await response.json(), PRODUCT_REPOSITORY_URL)
        if (active) {
          setReleases(rows)
          setPreview(!rows.some(row => !row.preview))
        }
      })
      .catch(() => {
        if (active) setError(true)
      })
      .finally(() => clearTimeout(timer))
    return () => {
      active = false
      clearTimeout(timer)
      controller.abort()
    }
  }, [revision])
  const release = releases?.find(row => row.preview === preview)
  return (
    <main className="h-dvh overflow-y-auto bg-[var(--background)] text-[var(--foreground)]">
      <div className="mx-auto max-w-5xl px-6 py-8 sm:py-12">
        <nav
          className="mb-10 flex flex-wrap items-center justify-between gap-4"
          aria-label={t('Navigation')}
        >
          <Link href="/learning" className="flex items-center gap-2 font-semibold">
            <Image src={BRAND_ASSETS.logo} alt="" width={26} height={26} />
            Lorevia
          </Link>
          <Link href="/learning" className="text-sm underline">
            {t('Back to workspace')}
          </Link>
        </nav>
        <h1 className="text-3xl font-semibold">{t('Download desktop app')}</h1>
        <p className="mt-3 max-w-2xl text-sm leading-6 text-[var(--muted-foreground)]">
          {t(
            'Choose your operating system. Download links appear only after an installer has been published.'
          )}
        </p>
        <div className="my-5 flex flex-wrap items-center gap-3">
          <button className={pageActionClass('secondary')} onClick={() => setRevision(v => v + 1)}>
            {t('Refresh downloads')}
          </button>
          {releases?.some(row => row.preview) && releases.some(row => !row.preview) && (
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={preview}
                onChange={e => setPreview(e.target.checked)}
              />
              {t('Show preview builds')}
            </label>
          )}
        </div>
        {error ? (
          <p role="alert" className="mb-5 rounded-lg border border-red-500/40 p-4 text-sm">
            {t('Unable to check desktop downloads. Retry or open GitHub Releases below.')}
          </p>
        ) : releases === null ? (
          <p role="status" className="mb-5 text-sm">
            {t('Checking desktop downloads…')}
          </p>
        ) : !release ? (
          <p role="status" className="mb-5 rounded-xl border border-[var(--border)] p-4 text-sm">
            {t(
              'Desktop installers have not been published yet. The source download below requires setup and is not a desktop installer.'
            )}
          </p>
        ) : (
          <div className="mb-5 text-sm">
            <a href={release.url} target="_blank" rel="noopener noreferrer" className="underline">
              {release.name}
            </a>
            {release.preview && (
              <span className="ml-3 rounded-full bg-amber-500/10 px-2.5 py-1 text-xs text-amber-800 dark:text-amber-300">
                {t('Preview build')}
              </span>
            )}
          </div>
        )}
        <div className="grid gap-4 md:grid-cols-3">
          {platforms.map(platform => {
            const assets = release?.assets.filter(a => a.platform === platform.id) ?? []
            return (
              <section
                key={platform.id}
                aria-label={platform.name}
                className="min-w-0 rounded-xl border border-[var(--border)] p-5"
              >
                <h2 className="font-semibold">{platform.name}</h2>
                <p className="mb-5 mt-2 text-sm text-[var(--muted-foreground)]">
                  {t(platform.description)}
                </p>
                <div className="space-y-3">
                  {assets.length ? (
                    assets.map(asset => (
                      <div key={asset.url}>
                        <a
                          href={asset.url}
                          className={`${pageActionClass()} w-full`}
                          rel="noopener noreferrer"
                        >
                          {t(
                            asset.name.endsWith('Portable.zip')
                              ? 'Download portable ZIP'
                              : asset.name.endsWith('.exe')
                                ? 'Download installer'
                                : 'Download DMG'
                          )}
                        </a>
                        <p className="mt-2 break-all text-xs text-[var(--muted-foreground)]">
                          {asset.name} · {(asset.size / 1024 / 1024).toFixed(1)} MiB
                        </p>
                      </div>
                    ))
                  ) : (
                    <p className="rounded-lg bg-[var(--muted)] p-3 text-sm text-[var(--muted-foreground)]">
                      {error
                        ? t('Download status unavailable')
                        : releases === null
                          ? t('Checking…')
                          : t('Not available yet')}
                    </p>
                  )}
                </div>
              </section>
            )
          })}
        </div>
        <footer className="mt-8 flex flex-wrap gap-5 border-t border-[var(--border)] pt-5 text-sm">
          <a
            href={PRODUCT_RELEASES_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="underline"
          >
            {t('All releases on GitHub')}
          </a>
          <a
            href={`${PRODUCT_REPOSITORY_URL}/archive/refs/heads/master.tar.gz`}
            className="underline"
          >
            {t('Download source code (requires setup)')}
          </a>
        </footer>
      </div>
    </main>
  )
}
