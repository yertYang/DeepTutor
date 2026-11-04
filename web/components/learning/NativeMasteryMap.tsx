'use client'
import { useTranslation } from 'react-i18next'
type Unit = { id: string; title: string; prerequisites: number[] }
type Summary = {
  objective_id: string
  status: string
  round_correct: number
  round_required: number
}
const labels: Record<string, string> = {
  locked: 'Prerequisites not completed',
  ready: 'Ready to practice',
  practicing: 'Practicing',
  mastered: 'Round completed',
  review_due: 'Review due now',
  review_scheduled: 'Review scheduled after a lapse',
}
const colors: Record<string, string> = {
  locked: '#78716c',
  ready: '#2563eb',
  practicing: '#b45309',
  mastered: '#15803d',
  review_due: '#be123c',
  review_scheduled: '#9d174d',
}
function fitTitle(title: string): string {
  let width = 0,
    result = ''
  for (const char of title) {
    width += (char.codePointAt(0) ?? 0) >= 0x2e80 ? 2 : 1
    if (width > 24) return result + '…'
    result += char
  }
  return result
}
export default function NativeMasteryMap({
  units,
  progress,
  onSelect,
}: {
  units: Unit[]
  progress: Summary[]
  onSelect: (id: string) => void
}) {
  const { t } = useTranslation()
  const levels: number[] = []
  const rows = new Map<number, number>()
  const points = units.map((unit, index) => {
    const level = unit.prerequisites.reduce((depth, i) => Math.max(depth, (levels[i] ?? 0) + 1), 0)
    levels[index] = level
    const row = rows.get(level) ?? 0
    rows.set(level, row + 1)
    return { x: 18 + level * 230, y: 18 + row * 110 }
  })
  const width = Math.max(320, (Math.max(0, ...levels) + 1) * 230 + 18)
  const height = Math.max(130, Math.max(1, ...rows.values()) * 110 + 18)
  return (
    <section className="my-4" aria-label={t('Mastery map')}>
      <h3 className="mb-2 text-sm font-semibold">{t('Mastery map')}</h3>
      <p className="mb-2 text-xs text-[var(--muted-foreground)]">
        {t('Select a goal to view its evidence and prerequisites.')}
      </p>
      <div
        className="max-h-[32rem] overflow-auto rounded-xl border border-[var(--border)]"
        tabIndex={0}
        role="region"
        aria-label={t('Learning objective map')}
      >
        <svg
          width={width}
          height={height}
          role="group"
          aria-label={t('Learning objectives and prerequisite connections')}
        >
          {units.flatMap((unit, i) =>
            unit.prerequisites.map(parent => {
              const a = points[parent],
                b = points[i]
              return a && b ? (
                <path
                  key={`${parent}-${i}`}
                  d={`M ${a.x + 200} ${a.y + 40} C ${a.x + 220} ${a.y + 40}, ${b.x - 20} ${b.y + 40}, ${b.x} ${b.y + 40}`}
                  fill="none"
                  stroke="var(--muted-foreground)"
                  strokeWidth={1.5}
                  opacity={0.6}
                />
              ) : null
            })
          )}
          {units.map((unit, index) => {
            const point = points[index]
            const summary = progress.find(p => p.objective_id === unit.id)
            const status = summary?.status ?? 'ready'
            const label = t(labels[status] ?? 'Ready to practice')
            return (
              <g
                key={unit.id}
                role="button"
                tabIndex={0}
                aria-label={`${unit.title}: ${label}`}
                onClick={() => onSelect(unit.id)}
                onKeyDown={event => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault()
                    onSelect(unit.id)
                  }
                }}
                className="cursor-pointer outline-none focus-visible:[&>rect]:stroke-[3px]"
              >
                <title>
                  {unit.title} · {label}
                </title>
                <rect
                  x={point.x}
                  y={point.y}
                  width={200}
                  height={80}
                  rx={10}
                  fill="var(--background)"
                  stroke={colors[status] ?? colors.ready}
                  strokeWidth={1.5}
                />
                <text
                  x={point.x + 12}
                  y={point.y + 25}
                  fill="var(--foreground)"
                  fontSize={13}
                  fontWeight={600}
                >
                  {fitTitle(unit.title)}
                </text>
                <text
                  x={point.x + 12}
                  y={point.y + 47}
                  fill={colors[status] ?? colors.ready}
                  fontSize={11}
                >
                  {fitTitle(label)}
                </text>
                {summary && (
                  <text
                    x={point.x + 12}
                    y={point.y + 66}
                    fill="var(--muted-foreground)"
                    fontSize={11}
                  >
                    {t(summary.status === 'mastered' ? 'Next round' : 'Fresh correct answers')}:{' '}
                    {summary.round_correct}/{summary.round_required}
                  </text>
                )}
              </g>
            )
          })}
        </svg>
      </div>
    </section>
  )
}
