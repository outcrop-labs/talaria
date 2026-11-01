<script lang="ts">
  import { AtSign, Bell, BellOff } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import {
    LEVELS,
    MUTE_PRESETS,
    isMuted,
    levelLabel,
    muteLabel,
    muteUntil,
    mutedLabel,
    putChannelNotify,
    readChannelNotify,
    type ChannelLevel,
    type ChannelNotifyPatch,
  } from '@/lib/channel-notify'

  // A conversation header's notification control (beside the star): how much
  // of this channel or DM the person hears — All / Mentions / Nothing — and a
  // timed mute. The server holds both on their own membership row and the
  // producers obey them (lib/channel-notify.ts says how). Hidden when the api
  // serves no `notify` (an older server), and when the person has no row to
  // hold a level of their own (a task room, a team grant: `settable` false).
  let { channelId, detail }: { channelId: string; detail: unknown } = $props()

  const qc = useQueryClient()
  const served = $derived(readChannelNotify(detail))
  // The last write's answer stands in for the detail until it refetches, so
  // the header never flickers back to the old state between the two.
  let written = $state<{ id: string; level: ChannelLevel; mutedUntil: string | null } | null>(null)
  const current = $derived(
    served && written?.id === channelId ? { ...served, level: written.level, mutedUntil: written.mutedUntil } : served,
  )
  // A mute runs out on its own: the label re-reads the clock each half minute.
  let now = $state(Date.now())
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 30_000)
    return () => clearInterval(t)
  })
  const muted = $derived(isMuted(current, now))
  const mutedLine = $derived(mutedLabel(current, now))
  let saving = $state(false)

  async function write(patch: ChannelNotifyPatch) {
    if (saving) return
    saving = true
    try {
      const r = await putChannelNotify(channelId, patch)
      written = { id: channelId, ...r }
      now = Date.now()
      await qc.invalidateQueries({ queryKey: ['channel', channelId] })
    } catch (e) {
      await alert({ title: 'Notifications not changed', message: errorMessage(e) })
    } finally {
      saving = false
    }
  }

  const items = (): ContextMenuEntry[] => {
    const level = current?.level ?? 'mentions'
    const entries: ContextMenuEntry[] = LEVELS.map((l) => ({
      label: levelLabel(l),
      checked: l === level,
      onSelect: () => void write({ level: l }),
    }))
    entries.push('sep')
    for (const p of MUTE_PRESETS) {
      entries.push({ label: muteLabel(p), onSelect: () => void write({ mutedUntil: muteUntil(p, Date.now()) }) })
    }
    if (muted) entries.push({ label: 'Unmute', onSelect: () => void write({ mutedUntil: null }) })
    return entries
  }

  const title = $derived(
    mutedLine ?? (current ? `Notifications: ${levelLabel(current.level).toLowerCase()}` : 'Notifications'),
  )
</script>

{#if current?.settable}
  <div class="flex min-w-0 items-center gap-1.5" data-testid="channel-notify">
    <DropdownMenu align="left" {items}>
      {#snippet trigger(open)}
        <IconButton
          size="sm"
          {title}
          aria-label={title}
          aria-haspopup="menu"
          active={open}
          disabled={saving}
          data-level={current.level}
          data-muted={muted ? 'true' : 'false'}
          class={cn((muted || current.level === 'none') && 'text-muted')}
        >
          {#if muted || current.level === 'none'}
            <BellOff size={15} />
          {:else if current.level === 'mentions'}
            <AtSign size={15} />
          {:else}
            <Bell size={15} />
          {/if}
        </IconButton>
      {/snippet}
    </DropdownMenu>
    {#if mutedLine}
      <span class="truncate text-xs text-muted" data-testid="channel-notify-muted">{mutedLine}</span>
    {/if}
  </div>
{/if}
