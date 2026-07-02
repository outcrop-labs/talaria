import { cn } from '@/lib/cn'

// Brand-colored monogram marks for model providers (not trademarked artwork —
// clean initials on the provider's signature color).
const MARKS: Record<string, { bg: string; fg: string; glyph: string }> = {
  anthropic: { bg: '#D97757', fg: '#1a1a18', glyph: 'A' },
  openai: { bg: '#10a37f', fg: '#ffffff', glyph: '◯' },
  openrouter: { bg: '#6467f2', fg: '#ffffff', glyph: 'OR' },
  deepseek: { bg: '#4d6bfe', fg: '#ffffff', glyph: 'D' },
  'x-ai': { bg: '#000000', fg: '#ffffff', glyph: '𝕏' },
  google: { bg: '#4285F4', fg: '#ffffff', glyph: 'G' },
  mistral: { bg: '#ff7000', fg: '#ffffff', glyph: 'M' },
  groq: { bg: '#f55036', fg: '#ffffff', glyph: 'g' },
  ollama: { bg: '#ffffff', fg: '#000000', glyph: '🦙' },
  vllm: { bg: '#fcb92c', fg: '#30302e', glyph: 'v' },
  litellm: { bg: '#2e8bff', fg: '#ffffff', glyph: 'L' },
}

/** A provider's mark; unknown providers get a neutral monogram of their name. */
export function ProviderMark({ provider, name, className }: { provider: string; name?: string; className?: string }) {
  const key = provider.toLowerCase()
  // Endpoint names often say more than provider ('custom'): try name first.
  const m = MARKS[(name ?? '').toLowerCase()] ?? MARKS[key]
  const glyph = m?.glyph ?? (name ?? provider).charAt(0).toUpperCase()
  return (
    <span
      aria-hidden
      className={cn('grid h-5 w-5 shrink-0 place-items-center rounded-md text-[10px] font-bold', className)}
      style={{
        background: m?.bg ?? 'var(--theme-line)',
        color: m?.fg ?? 'var(--theme-fg)',
        border: m?.bg === '#ffffff' ? '1px solid var(--theme-line)' : undefined,
      }}
    >
      {glyph}
    </span>
  )
}
