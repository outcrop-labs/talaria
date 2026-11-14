<script lang="ts">
  import type { Snippet } from 'svelte'
  import { publicIconDomain } from '@/lib/icon-domain'
  import { useUiVoice } from '@/lib/ui-voice'

  /** A server's mark. The registry-declared icon hotlinks DIRECTLY (fast, no
   *  middleman); the cached favicon proxy is the fallback, one host at a time
   *  (`iconDomains` orders them); a letter tile is the floor. */
  let {
    title,
    domain,
    icon,
    size = 32,
    children,
  }: {
    title: string
    /** The host to ask the icon proxy for, or several to try in order. */
    domain?: string | readonly string[] | null
    icon?: string | null
    size?: number
    /** A glyph of the app's own, for a server that is a part of Talaria: it
     *  takes the plate a brand mark sits on. Settings voice only. */
    children?: Snippet
  } = $props()

  // Internal hosts have no favicon — don't ask the proxy (it can only 404);
  // the letter tile paints immediately instead.
  const sources = $derived(
    [
      icon,
      ...[domain ?? []]
        .flat()
        .map((d) => publicIconDomain(d))
        .filter((d): d is string => d !== null)
        .map((d) => `/api/mcp/icon?domain=${encodeURIComponent(d)}`),
    ].filter((u): u is string => !!u),
  )
  // By address, so a row that is handed another server starts from its own list.
  let failed = $state<string[]>([])
  const src = $derived(sources.find((u) => !failed.includes(u)) ?? null)
  const miss = () => {
    if (src) failed = [...failed, src]
  }
  // The agent dialog draws this in comms too, where the voice is mono: that
  // branch is the mark as it was.
  const sans = useUiVoice() === 'sans'
  const monogram = $derived((title[0] ?? '?').toUpperCase())
</script>

{#if sans}
  <!-- The settings area's entity tile, the one `SettingsRow` draws for an icon.
       A mark and an initial share it, so a list that has both reads as one
       set, and the initial reads as a letter someone set there and never as
       an image that failed. -->
  <span class="plate grid h-8 w-8 shrink-0 place-items-center overflow-hidden rounded-md border border-line bg-raised text-fg">
    {#if children}
      {@render children()}
    {:else if src}
      <img {src} alt="" width="20" height="20" loading="lazy" onerror={miss} class="h-5 w-5 object-contain" />
    {:else}
      <span aria-hidden="true" class="font-sans text-sm font-medium leading-none">{monogram}</span>
    {/if}
  </span>
{:else if !src}
  <span
    class="grid shrink-0 place-items-center rounded-md border border-line-subtle bg-raised font-mono font-semibold text-muted"
    style:width="{size}px"
    style:height="{size}px"
    style:font-size="{size * 0.45}px"
  >
    {monogram}
  </span>
{:else}
  <img
    {src}
    alt=""
    width={size}
    height={size}
    loading="lazy"
    onerror={miss}
    class="shrink-0 rounded-md bg-raised object-contain"
    style:width="{size}px"
    style:height="{size}px"
  />
{/if}

<style>
  /* Service marks are drawn for a light page: GitHub's is black on nothing.
     In the dark theme the tile is a light plate so a mark can be seen, and an
     initial takes the same plate in the ground's ink. */
  :global([data-theme='mercury']) .plate {
    border-color: transparent;
    background: color-mix(in srgb, var(--theme-text) 90%, var(--theme-panel));
    color: var(--theme-bg);
  }
</style>
