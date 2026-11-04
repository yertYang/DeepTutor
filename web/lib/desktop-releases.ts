export type DesktopPlatform = 'macos-arm64' | 'macos-x64' | 'windows-x64'
export type DesktopAsset = { name: string; url: string; size: number; platform: DesktopPlatform }
export type DesktopRelease = {
  name: string
  tag: string
  url: string
  preview: boolean
  assets: DesktopAsset[]
}

function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}
function platform(name: string): DesktopPlatform | null {
  if (/^Lorevia-MacOS-Apple-Silicon-v.+\.dmg$/.test(name)) return 'macos-arm64'
  if (/^Lorevia-MacOS-Intel-v.+\.dmg$/.test(name)) return 'macos-x64'
  if (/^Lorevia-Windows-v.+-(Setup\.exe|Portable\.zip)$/.test(name)) return 'windows-x64'
  return null
}
/** Only published, complete installer assets in this repository can become download links. */
export function desktopReleases(value: unknown, repository: string): DesktopRelease[] {
  if (!Array.isArray(value)) throw new Error('Invalid desktop release list')
  const root = new URL(repository)
  if (root.protocol !== 'https:' || root.hostname !== 'github.com')
    throw new Error('Invalid repository')
  const prefix = root.pathname.replace(/\/$/, '') + '/releases/'
  const validUrl = (value: unknown, path: string) => {
    if (typeof value !== 'string') return false
    try {
      const url = new URL(value)
      return (
        url.origin === root.origin &&
        !url.username &&
        !url.password &&
        url.pathname.startsWith(prefix + path)
      )
    } catch {
      return false
    }
  }
  return value.flatMap(release => {
    if (
      !record(release) ||
      release.draft !== false ||
      typeof release.tag_name !== 'string' ||
      !validUrl(release.html_url, 'tag/') ||
      !Array.isArray(release.assets)
    )
      return []
    const assets = release.assets.flatMap(asset => {
      if (
        !record(asset) ||
        typeof asset.name !== 'string' ||
        asset.state !== 'uploaded' ||
        typeof asset.size !== 'number' ||
        !Number.isSafeInteger(asset.size) ||
        asset.size <= 0 ||
        !validUrl(asset.browser_download_url, 'download/')
      )
        return []
      const target = platform(asset.name)
      return target
        ? [
            {
              name: asset.name,
              url: asset.browser_download_url as string,
              size: asset.size,
              platform: target,
            },
          ]
        : []
    })
    return assets.length
      ? [
          {
            name:
              typeof release.name === 'string' && release.name ? release.name : release.tag_name,
            tag: release.tag_name,
            url: release.html_url as string,
            preview: release.prerelease === true,
            assets,
          },
        ]
      : []
  })
}
