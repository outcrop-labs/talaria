import { useEffect, useMemo, useRef, useState } from 'react'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/cn'

// A full, searchable emoji picker. The ~1900-emoji dataset (@emoji-mart/data) is
// lazy-loaded on first open so it never weighs down the initial bundle. Renders
// as a click-away popover; the caller positions the trigger.

interface EmojiEntry {
  id: string
  native: string
  name: string
  keywords: string[]
}
let CACHE: EmojiEntry[] | null = null
let CATEGORIES: Array<{ id: string; label: string; ids: string[] }> = []

const CATEGORY_LABEL: Record<string, string> = {
  people: 'Smileys & People',
  nature: 'Animals & Nature',
  foods: 'Food & Drink',
  activity: 'Activity',
  places: 'Travel & Places',
  objects: 'Objects',
  symbols: 'Symbols',
  flags: 'Flags',
}

// A tiny fallback so the picker still works if the dataset import ever fails.
const FALLBACK = '📄📝📌📚📁🗂️🧭🧠💡⚙️🚀🔧🔒🔑🎯✅📊📈🧪🗺️🏷️💬📣🌐🔍⭐🔥❤️⚠️🐛🤖👤🏢💰📅🎨'.split('')

async function loadEmoji(): Promise<EmojiEntry[]> {
  if (CACHE) return CACHE
  try {
    const data = (await import('@emoji-mart/data')).default as {
      emojis: Record<string, { id: string; name: string; keywords: string[]; skins: Array<{ native: string }> }>
      categories: Array<{ id: string; emojis: string[] }>
    }
    const entries: EmojiEntry[] = []
    for (const key of Object.keys(data.emojis)) {
      const e = data.emojis[key]!
      const native = e.skins?.[0]?.native
      if (native) entries.push({ id: e.id, native, name: e.name, keywords: e.keywords ?? [] })
    }
    CATEGORIES = data.categories.map((c) => ({ id: c.id, label: CATEGORY_LABEL[c.id] ?? c.id, ids: c.emojis }))
    CACHE = entries
  } catch {
    CACHE = FALLBACK.map((native, i) => ({ id: `f${i}`, native, name: native, keywords: [] }))
    CATEGORIES = [{ id: 'common', label: 'Common', ids: CACHE.map((e) => e.id) }]
  }
  return CACHE
}

export function EmojiPicker({
  onPick,
  onClear,
  onClose,
  align = 'left',
}: {
  onPick: (emoji: string) => void
  onClear?: () => void
  onClose: () => void
  align?: 'left' | 'right'
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [all, setAll] = useState<EmojiEntry[]>(CACHE ?? [])
  const [q, setQ] = useState('')

  useEffect(() => {
    if (!CACHE) void loadEmoji().then(setAll)
  }, [])
  useEffect(() => {
    const h = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    document.addEventListener('mousedown', h)
    return () => document.removeEventListener('mousedown', h)
  }, [onClose])

  const byId = useMemo(() => new Map(all.map((e) => [e.id, e])), [all])
  const results = useMemo(() => {
    const term = q.trim().toLowerCase()
    if (!term) return null // null → show categorized browse view
    return all
      .filter((e) => e.name.toLowerCase().includes(term) || e.id.includes(term) || e.keywords.some((k) => k.includes(term)))
      .slice(0, 96)
  }, [q, all])

  return (
    <div
      ref={ref}
      className={cn(
        'absolute top-full z-30 mt-1 w-72 rounded-xl border border-line bg-card p-2 shadow-lg',
        align === 'right' ? 'right-0' : 'left-0',
      )}
    >
      <Input autoFocus size="sm" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search emoji…" className="mb-2" />
      <div className="max-h-64 overflow-y-auto">
        {all.length === 0 ? (
          <div className="py-8 text-center text-xs text-muted">Loading…</div>
        ) : results ? (
          results.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted">No emoji found.</div>
          ) : (
            <Grid entries={results} onPick={onPick} />
          )
        ) : (
          CATEGORIES.map((cat) => {
            const entries = cat.ids.map((id) => byId.get(id)).filter(Boolean) as EmojiEntry[]
            if (entries.length === 0) return null
            return (
              <div key={cat.id} className="mb-1">
                <div className="px-1 py-1 text-[10px] uppercase tracking-wide text-muted">{cat.label}</div>
                <Grid entries={entries} onPick={onPick} />
              </div>
            )
          })
        )}
      </div>
      {onClear && (
        <button type="button" onClick={onClear} className="mt-1 w-full rounded-md py-1 text-[11px] text-muted hover:text-fg">
          Remove icon
        </button>
      )}
    </div>
  )
}

function Grid({ entries, onPick }: { entries: EmojiEntry[]; onPick: (e: string) => void }) {
  return (
    <div className="grid grid-cols-8 gap-0.5">
      {entries.map((e) => (
        <button key={e.id} type="button" title={e.name} onClick={() => onPick(e.native)} className="grid h-8 place-items-center rounded text-lg hover:bg-sidebar">
          {e.native}
        </button>
      ))}
    </div>
  )
}
