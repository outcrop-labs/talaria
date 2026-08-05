<script lang="ts">
  import { publicIconDomain } from '@/lib/icon-domain'

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
</script>

<!-- Publisher marks sit in retoned raised tiles (spec: retone containers only). -->
{#if !src || failed}
  <span
    class="grid shrink-0 place-items-center rounded-md border border-line-subtle bg-raised font-mono font-semibold text-muted"
    style:width="{size}px"
    style:height="{size}px"
    style:font-size="{size * 0.45}px"
  >
    {(title[0] ?? '?').toUpperCase()}
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
