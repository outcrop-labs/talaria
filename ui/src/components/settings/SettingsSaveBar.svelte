<script lang="ts">
  import { Check } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { useExplicitSave } from './autosave.svelte'

  // The foot of a group that saves with a button: why it needs one on the
  // left, and on the right, once there is something to save, the button named
  // for what it saves. With nothing to save there is no button, so the bar
  // never shows a Save that looks switched off. The last child of its
  // `SettingsGroup`. While it is mounted the page's header shows no
  // "Changes save automatically", so the two can never be on screen together.
  let {
    label,
    reason,
    dirty,
    saving = false,
    saved = false,
    disabled = false,
    onSave,
    onDiscard,
    class: className,
  }: {
    /** Names what it saves: "Save identity", "Save limits". */
    label: string
    /** One quiet sentence: why these need a Save ("Restarts the agent."). */
    reason?: string
    /** There is something to save: the gold button and Discard show. */
    dirty: boolean
    saving?: boolean
    /** The save just landed: a check and "Saved" take the sentence's place. */
    saved?: boolean
    /** Dirty but not savable yet (a required field is empty). */
    disabled?: boolean
    onSave: () => void
    /** Puts the fields back. Discard shows only with this, while dirty. */
    onDiscard?: () => void
    class?: string
  } = $props()

  useExplicitSave()
</script>

<!-- 57px: the buttons' row, its padding and the hairline above, so the bar is one height with and without them. -->
<div data-settings-row="apart" data-save-bar class={cn('flex min-h-[57px] flex-wrap items-center gap-x-6 gap-y-3 px-5 py-3', className)}>
  <p role="status" class="flex min-w-0 flex-1 basis-48 items-center gap-1.5 font-sans text-[13px] leading-5 text-muted">
    {#if saved && !dirty}<Check size={14} aria-hidden="true" class="shrink-0 text-success" />Saved{:else}{reason}{/if}
  </p>
  {#if dirty}
    <div class="flex shrink-0 items-center gap-2">
      {#if onDiscard}<Button variant="ghost" size="sm" disabled={saving} onclick={onDiscard}>Discard</Button>{/if}
      <Button size="sm" disabled={saving || disabled} onclick={onSave}>{label}</Button>
    </div>
  {/if}
</div>
