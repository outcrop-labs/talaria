<script lang="ts">
  import { Check, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { iconDomains } from '@/lib/mcp-icon'
  import { relativeTime } from '@/lib/fleet'
  import { useUiVoice } from '@/lib/ui-voice'
  import McpServerMark from './McpServerMark.svelte'
  import { TIER_BADGE, connectPopup, deadLine, type LibraryServerRow, type MarketplaceAdded } from './mcp'

  /** One catalog entry in the marketplace grid: who publishes it, what it is
   *  for, and one control — add it, or the mark that says it is already here. */
  let {
    entry: l,
    added,
    serverId,
    onAgent,
    busy,
    onInstall,
    onPick,
    onSetCredentials,
  }: {
    entry: LibraryServerRow
    /** Set when this sitting installed it. */
    added?: MarketplaceAdded
    /** Its row in the org registry, when it has one. */
    serverId?: string
    /** The agent the marketplace was opened for already carries it. */
    onAgent: boolean
    busy: boolean
    onInstall: () => void
    /** Opened for an agent: attach a server the org already has. */
    onPick?: (serverId: string) => void
    onSetCredentials: (serverId: string) => void
  } = $props()

  // An agent's dialog opens the marketplace from comms too, where the voice is
  // mono: the second template below is the card as it was.
  const sans = useUiVoice() === 'sans'
  const installed = $derived(!!serverId)
  const badge = $derived(TIER_BADGE[l.tier])
  const dead = $derived(deadLine(l.live, relativeTime))
  const wantsKey = $derived(l.requiredHeaders.length > 0 || !!l.package?.declaredEnv.length)
  const addTitle = $derived(
    l.package ? 'Add to your org (runs a container)' : l.requiredHeaders.length ? 'Add to your org (needs credentials)' : 'Add to your org',
  )
  const pkgHint = sans
    ? 'Ships as a package. Talaria runs it in a locked-down container, and only an admin can install it.'
    : 'Ships as a package (npm/pypi/docker) — Talaria runs it in a hardened container (unprivileged, read-only, no host mounts); admins install these'
  const keyHint = sans ? 'You will be asked for its credentials.' : "Declares the credentials it needs — you'll be asked for them"
</script>

{#if sans}
  <div data-mcp-market={l.registryName} class={cn('flex flex-col rounded-lg border border-line p-4 transition-colors hover:border-line-strong', dead && 'opacity-70')}>
    <div class="flex items-center gap-3">
      <McpServerMark title={l.title} domain={iconDomains({ publisher: l.domain, url: l.url })} icon={l.icon} />
      <div class="min-w-0 flex-1">
        <div class="truncate font-sans text-sm font-medium leading-5 text-fg">{l.title}</div>
        {#if l.domain}<div class="truncate font-mono text-xs leading-5 text-muted">{l.domain}</div>{/if}
      </div>
      {#if installed && onPick && !onAgent && serverId}
        <Button size="xs" variant="outline" onclick={() => onPick(serverId)} title="Already in the org registry. Add it to this agent">Use</Button>
      {:else if installed}
        <span
          class="flex shrink-0 items-center gap-1.5 font-sans text-xs leading-4 text-muted"
          title={onPick ? 'Already on this agent' : 'In your org registry'}
        >
          <Check size={14} class="text-success" />Added
        </span>
      {:else}
        <Button size="xs" variant="outline" class="gap-1.5" title={addTitle} aria-label={addTitle} disabled={busy} onclick={onInstall}>
          {#if busy}<WaitingMark site="mcp/install" size={12} />{/if}
          Add
        </Button>
      {/if}
    </div>
    <p class="mt-3 line-clamp-2 min-h-10 font-sans text-[13px] leading-5 text-muted">{l.description ?? 'No description.'}</p>
    <div class="mt-3 flex flex-wrap items-center gap-1.5">
      <Chip tone={badge.tone} title={badge.hint}>{badge.word}</Chip>
      {#if l.package}<Chip title={pkgHint}>Package</Chip>{/if}
      {#if wantsKey}<Chip title={keyHint}>Needs a key</Chip>{/if}
    </div>
    {#if dead}<p class="mt-2 font-sans text-xs leading-4 text-muted" title={l.live?.detail ?? undefined}>{dead}</p>{/if}
    {#if added && added.state !== 'ok'}
      <div class="mt-3 flex flex-wrap items-center gap-2 font-sans text-[13px] leading-5">
        {#if added.state === 'connect'}
          <span class="text-muted">Added. It signs in with OAuth.</span>
          <Button size="xs" variant="outline" onclick={() => void connectPopup(added.serverId, 'org')}>Connect org account</Button>
        {:else if added.state === 'needs-app'}
          <span class="text-warning">Added. Set up its OAuth app on the MCP page before anyone can connect.</span>
        {:else if added.state === 'needs-key'}
          <span class="text-warning">Added. It needs credentials.</span>
          <Button size="xs" variant="outline" onclick={() => onSetCredentials(added.serverId)}>Set credentials</Button>
        {:else}
          <span class="text-danger" title={added.detail}>Added, but the first check failed: {added.detail}</span>
        {/if}
      </div>
    {/if}
  </div>
{:else}
  <div
    class={cn(
      'group relative flex flex-col rounded-lg border border-line-subtle p-4 transition-colors hover:border-line dither-fill',
      dead && 'opacity-70',
    )}
  >
    <div class="flex items-center gap-3 pr-8">
      <McpServerMark title={l.title} domain={l.domain} icon={l.icon} size={34} />
      <div class="min-w-0 flex-1">
        <div class="truncate font-sans text-sm font-semibold text-fg">{l.title}</div>
        <div class="flex flex-wrap items-center gap-1.5">
          {#if l.domain}<span class="truncate font-mono text-[11px] text-muted">{l.domain}</span>{/if}<!-- voice-ok: mono branch -->
          <span title={badge.hint} class={cn('shrink-0 rounded border px-1.5 font-mono text-[10px] uppercase tracking-[0.05em]', badge.cls)}><!-- voice-ok: mono branch -->
            {badge.label}
          </span>
          {#if l.package}
            <span title={pkgHint} class="shrink-0 rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim"><!-- voice-ok: mono branch -->
              pkg
            </span>
          {/if}
          {#if wantsKey}
            <span title={keyHint} class="shrink-0 rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim"><!-- voice-ok: mono branch -->
              key
            </span>
          {/if}
        </div>
      </div>
    </div>
    <p class="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
      {l.description ?? 'No description.'}
    </p>
    {#if dead}
      <p class="mt-1.5 font-sans text-[11px] text-ink-dim" title={l.live?.detail ?? undefined}>{dead}</p><!-- voice-ok: mono branch -->
    {/if}
    {#if added && added.state !== 'ok'}
      <div class="mt-2 flex flex-wrap items-center gap-2 font-sans text-[11px]"><!-- voice-ok: mono branch -->
        {#if added.state === 'connect'}
          <span class="text-muted">Added. It uses OAuth —</span>
          <Button size="xs" variant="accent-soft" onclick={() => void connectPopup(added.serverId, 'org')}>Connect org account</Button>
        {:else if added.state === 'needs-app'}
          <span class="text-warning">Added. Needs a one-time OAuth app setup on the MCP page before anyone can connect.</span>
        {:else if added.state === 'needs-key'}
          <span class="text-warning">Added. It wants credentials —</span>
          <Button size="xs" variant="accent-soft" onclick={() => onSetCredentials(added.serverId)}>Set credentials</Button>
        {:else}
          <span class="text-danger" title={added.detail}>Added, but discovery failed: {added.detail}</span>
        {/if}
      </div>
    {/if}
    <!-- The install control: quiet until the card is engaged. -->
    <div class="absolute right-3 top-3">
      {#if installed && onPick && !onAgent && serverId}
        <Button size="xs" variant="outline" onclick={() => onPick(serverId)} title="Already in the org registry — attach it to this agent">
          Use
        </Button>
      {:else if installed}
        <span class="grid h-7 w-7 place-items-center rounded-full bg-success/10 text-success" title={onPick ? 'Already on this agent' : 'In your org registry'}>
          <Check size={14} />
        </span>
      {:else}
        <button
          type="button"
          title={addTitle}
          disabled={busy}
          onclick={onInstall}
          class={cn(
            'grid h-7 w-7 place-items-center rounded-full border transition-all',
            busy
              ? 'border-line bg-raised text-muted opacity-100'
              : 'border-line bg-raised text-muted opacity-0 hover:border-accent hover:text-accent group-hover:opacity-100',
          )}
        >
          {#if busy}<WaitingMark site="mcp/install" size={12} />{:else}<Plus size={14} />{/if}
        </button>
      {/if}
    </div>
  </div>
{/if}
