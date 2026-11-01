<script lang="ts">
  import { AtSign, Bell, BellOff } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
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
    refusalMessage,
    withNotify,
    type ChannelNotifyPatch,
  } from '@/lib/channel-notify'

  // A conversation header's notification control: how much of this channel or
  // DM the person hears — All / Mentions / Nothing — and a timed mute. The
  // server holds both on their own membership row and the producers obey them
  // (lib/channel-notify.ts says how). Hidden when the api serves no `notify`
  // (an older server), and when the person has no row to hold a level of
  // their own (a task room, a team grant: `settable` false).
  //
  // Two parts, one source: `control` (the bell button and its menu, beside the
  // star) and `status` (the "Muted until …" line, after the conversation's
  // name — the header reads star, name, then status). Both read the channel
  // detail query, and a write puts the server's answer straight into that
  // query, so the two never disagree and nothing local outlives a refetch.
  let {
    channelId,
    detail,
    part = 'control',
  }: { channelId: string; detail: unknown; part?: 'control' | 'status' } = $props()

  const qc = useQueryClient()
  const current = $derived(readChannelNotify(detail))
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
      qc.setQueryData(['channel', channelId], (old: unknown) => withNotify(old, r))
      now = Date.now()
      await qc.invalidateQueries({ queryKey: ['channel', channelId] })
    } catch (e) {
      await alert({ title: 'Notifications not changed', message: refusalMessage(e) })
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
      entries.push({
        label: muteLabel(p),
        icon: [BellOff, { size: 14 }],
        onSelect: () => void write({ mutedUntil: muteUntil(p, Date.now()) }),
      })
    }
    if (muted) entries.push({ label: 'Unmute', icon: [Bell, { size: 14 }], onSelect: () => void write({ mutedUntil: null }) })
    return entries
  }

  const title = $derived(
    mutedLine ?? (current ? `Notifications: ${levelLabel(current.level).toLowerCase()}` : 'Notifications'),
  )
</script>

{#if current?.settable}
  {#if part === 'control'}
    <div class="flex shrink-0 items-center" data-testid="channel-notify">
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
    </div>
  {:else if mutedLine}
    <!-- The line takes only the header's leftover room (basis 0, grow), so in
         a tight header it gives way first — down to its icon, the full words
         in its tooltip and the bell's — and never squeezes the name. -->
    <span
      class="flex min-w-4 basis-0 grow items-center gap-1 overflow-hidden text-xs text-muted"
      data-testid="channel-notify-muted"
    >
      <BellOff size={12} class="shrink-0" aria-hidden="true" />
      <span class="truncate" title={mutedLine}>{mutedLine}</span>
    </span>
  {/if}
{/if}
