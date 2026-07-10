import { cn } from '@/lib/cn'
import { PROVIDER_LOGOS } from './provider-logos'

// Neutral monogram fallback for providers we have no real mark for (OpenAI, Groq,
// LiteLLM, Together, ). Kept monochrome to sit beside the real logos cleanly.
const MONOGRAM: Record<string, string> = {
  openai: 'AI',
  groq: 'gq',
  litellm: 'LL',
  together: 'T',
  fireworks: 'F',
  cerebras: 'C',
  deepinfra: 'DI',
}

// Map an endpoint's provider/name onto a known logo slug. Endpoint names often
// say more than the provider ('custom' covers Gemini, Groq, ), so try name first.
function resolve(provider: string, name?: string): { logo?: string; mono: string } {
  const keys = [name?.toLowerCase(), provider.toLowerCase()].filter(Boolean) as string[]
  for (const k of keys) if (PROVIDER_LOGOS[k]) return { logo: PROVIDER_LOGOS[k], mono: '' }
  for (const k of keys) if (MONOGRAM[k]) return { mono: MONOGRAM[k] }
  return { mono: (name ?? provider).charAt(0).toUpperCase() }
}

/** A provider's real monochrome brand mark; unknown providers get a neutral monogram. */
export function ProviderMark({ provider, name, className }: { provider: string; name?: string; className?: string }) {
  const { logo, mono } = resolve(provider, name)
  return (
    <span
      aria-hidden
      className={cn(
        'grid h-5 w-5 shrink-0 place-items-center rounded-md border border-line-subtle bg-card2 text-fg',
        className,
      )}
    >
      {logo ? (
        <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
          <path d={logo} />
        </svg>
      ) : (
        <span className="text-[10px] font-bold text-muted">{mono}</span>
      )}
    </span>
  )
}
