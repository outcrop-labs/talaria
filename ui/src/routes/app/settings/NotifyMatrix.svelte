<script lang="ts">
  import { Bell, Inbox, Mail, Monitor, RotateCcw } from '@lucide/svelte'
  import MenuValueButton from '@/components/notify/MenuValueButton.svelte'
  import {
    DELIVERY_CHANNELS,
    deliveredOn,
    deliverySummary,
    EMAIL_OFF_REASON,
    kindHint,
    PUSH_UNREGISTERED_REASON,
  } from '@/components/notify/delivery'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import type { ContextMenuEntry, MenuIcon } from '@/components/ui/context-menu.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import { useNotifyPatch } from '@/lib/notify-center'
  import { matrixGroups, mergePatch, resetToDefault, setChannel, type MatrixChannel, type MatrixRow } from '@/lib/notify-matrix'
  import type { ChannelsPatch, ChannelsPrefs, ClassInfo } from '@/lib/notify-wire'

  // WHERE EACH KIND GOES. Rows are the server's registry (`classes[]`, I-12)
  // grouped by category. Each row has one control: a menu button that reads
  // where the kind is delivered beyond the inbox, and opens the channels as
  // checkable items. A pick saves at once (`PATCH {channels}`), the label
  // moves with it, and a refusal rolls it back with the reason. The server
  // rewrites each class's legacy route from it (I-13), so an older app reading
  // `prefs` still behaves as close as it can.
  let {
    classes,
    channels,
    emailEnabled,
    pushRegistered = true,
    onSaved,
    onError,
  }: {
    classes: readonly ClassInfo[]
    channels: ChannelsPrefs | undefined
    /** The instance email switch (`delivery.emailEnabled`). */
    emailEnabled: boolean
    /** False once the account is known to have no browser registered for push. */
    pushRegistered?: boolean
    onSaved?: () => void
    onError?: (message: string) => void
  } = $props()

  const patch = useNotifyPatch()
  let pending = $state<ChannelsPatch>({})
  const groups = $derived(matrixGroups(classes, channels, pending))

  const save = async (change: ChannelsPatch) => {
    pending = mergePatch(pending, change)
    const r = await patch({ channels: change })
    // Drop exactly the keys this save sent, so a second click still in flight
    // keeps its overlay.
    const next: ChannelsPatch = { ...pending }
    for (const [id, v] of Object.entries(change)) {
      const left = { ...next[id] }
      for (const k of Object.keys(v)) delete left[k as keyof typeof left]
      if (Object.keys(left).length) next[id] = left
      else delete next[id]
    }
    pending = next
    if ('error' in r) onError?.(r.error)
    else onSaved?.()
  }

  const reason = (ch: MatrixChannel): string | undefined =>
    ch === 'email' && !emailEnabled ? EMAIL_OFF_REASON : ch === 'push' && !pushRegistered ? PUSH_UNREGISTERED_REASON : undefined

  // Read from `groups` when the menu draws, so an open menu follows each pick.
  const items = (id: string): ContextMenuEntry[] => {
    const r = groups.flatMap((g) => g.rows).find((row) => row.cls.id === id)
    if (!r) return []
    return [
      ...DELIVERY_CHANNELS.map((c): ContextMenuEntry => ({
        label: c.label,
        checked: r.channels[c.id],
        hint: reason(c.id),
        disabled: c.id === 'email' && !emailEnabled,
        keepOpen: true,
        onSelect: () => void save(setChannel(id, c.id, !r.channels[c.id])),
      })),
      // The icon fills the slot the checks use, so the label lines up with the channels.
      ...(r.atDefault
        ? []
        : ['sep' as const, { label: 'Reset to default', icon: [RotateCcw, { size: 14 }] as MenuIcon, onSelect: () => void save(resetToDefault(r.cls)) }]),
    ]
  }

  const mark = (on: readonly MatrixChannel[]) =>
    on.length === 0 ? Inbox : on.length > 1 ? Bell : on[0] === 'email' ? Mail : on[0] === 'desktop' ? Monitor : Bell
  const divided = '[&>*+*]:border-t [&>*+*]:border-line-subtle'
</script>

{#snippet control(r: MatrixRow)}
  {@const on = deliveredOn(r.channels, emailEnabled)}
  <DropdownMenu items={() => items(r.cls.id)} group="notify-delivery" align="left">
    {#snippet trigger(open)}
      <MenuValueButton
        {open}
        icon={mark(on)}
        label={deliverySummary(on)}
        aria-label="{r.cls.label}: {deliverySummary(on)}"
        data-matrix-menu
      />
    {/snippet}
  </DropdownMenu>
{/snippet}

<!-- Row-shaped content for a SettingsGroup: each category as a caption and
     its kinds as rows. -->
<div data-notify-matrix class={divided}>
  {#each groups as g (g.category)}
    <div class={divided} data-matrix-group={g.category}>
      <div class="bg-raised/40 px-5 py-2 font-sans text-xs font-medium leading-4 text-muted">{g.label}</div>
      <ul class={divided}>
        {#each g.rows as r (r.cls.id)}
          <li
            data-matrix-row={r.cls.id}
            data-desktop={String(r.channels.desktop)}
            data-push={String(r.channels.push)}
            data-email={String(r.channels.email)}
            data-at-default={String(r.atDefault)}
          >
            <SettingsRow label={r.cls.label} hint={kindHint(r.cls)}>
              {@render control(r)}
            </SettingsRow>
          </li>
        {/each}
      </ul>
    </div>
  {/each}
</div>
