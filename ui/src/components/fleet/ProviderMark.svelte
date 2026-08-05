<script lang="ts" module>
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
</script>

<script lang="ts">
  import { cn } from '@/lib/cn'

  /** A provider's real monochrome brand mark; unknown providers get a neutral monogram. */
  let { provider, name, class: className }: { provider: string; name?: string; class?: string } = $props()

  const resolved = $derived(resolve(provider, name))
</script>

<span
  aria-hidden="true"
  class={cn(
    'grid h-5 w-5 shrink-0 place-items-center rounded-md border border-line-subtle bg-card2 text-fg',
    className,
  )}
>
  {#if resolved.logo}
    <svg viewBox="0 0 24 24" fill="currentColor" class="h-3.5 w-3.5">
      <path d={resolved.logo} />
    </svg>
  {:else}
    <span class="text-[10px] font-bold text-muted">{resolved.mono}</span>
  {/if}
</span>
