<script lang="ts">
  import { publicIconDomain } from '@/lib/icon-domain'
  import { useUiVoice } from '@/lib/ui-voice'

  /** Publisher mark: the registry-declared icon hotlinks DIRECTLY (fast, no
   *  middleman); the cached favicon proxy is only the fallback; a monogram tile
   *  is the floor. */
  let {
    title,
    domain,
    icon,
    size = 32,
  }: { title: string; domain?: string | null; icon?: string | null; size?: number } = $props()

  let failed = $state(false)
  // Internal hosts have no favicon — don't ask the proxy (it can only 404);
  // the monogram tile paints immediately instead.
  const proxied = $derived(publicIconDomain(domain))
  const src = $derived(icon ?? (proxied ? `/api/mcp/icon?domain=${encodeURIComponent(proxied)}` : null))

  // The agent dialog draws this in comms too, where the voice is mono: that
  // branch is the mark as it was.
  const sans = useUiVoice() === 'sans'
  const monogram = $derived((title[0] ?? '?').toUpperCase())
</script>

{#if sans}
  <!-- The settings area's entity tile, the one `SettingsRow` draws for an icon:
       a monogram is the same tile with a letter in it, not a lesser mark. -->
  <span class="grid h-8 w-8 shrink-0 place-items-center overflow-hidden rounded-md border border-line bg-raised">
    {#if !src || failed}
      <span class="font-sans text-sm font-semibold leading-none text-muted">{monogram}</span>
    {:else}
      <img {src} alt="" width="20" height="20" loading="lazy" onerror={() => (failed = true)} class="h-5 w-5 object-contain" />
    {/if}
  </span>
{:else if !src || failed}
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
    onerror={() => (failed = true)}
    class="shrink-0 rounded-md bg-raised object-contain"
    style:width="{size}px"
    style:height="{size}px"
  />
{/if}
