<script lang="ts">
  import { Check } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import { useExplicitSave } from './autosave.svelte'

  // The foot of a group that saves with a button. It is not there until a
  // field changes: then why the group needs a Save shows on the left, and
  // Discard and the button named for what it saves on the right. When the
  // save lands it says so for as long as `saved` holds, then goes. The last
  // child of its `SettingsGroup`. While the component is mounted, bar or no
  // bar, the page's header shows no "Changes save automatically".
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
    /** There is something to save: the bar shows, with the gold button and Discard. */
    dirty: boolean
    saving?: boolean
    /** The save just landed: the bar stays, a check and "Saved" in the sentence's place. */
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

{#if dirty || saved}
  <!-- The wrapper slides and takes the group's hairline. The bar inside holds one height with and without its buttons,
       so "Saved" does not move the page a second time. -->
  <div transition:slide={{ duration: 150 }}>
    <div data-settings-row="apart" data-save-bar class={cn('flex min-h-14 flex-wrap items-center gap-x-6 gap-y-3 px-5 py-3', className)}>
      <p role="status" class="flex min-w-0 flex-1 basis-48 items-center gap-1.5 font-sans text-[13px] leading-5 text-muted">
        {#if dirty}{reason}{:else}<Check size={14} aria-hidden="true" class="shrink-0 text-success" />Saved{/if}
      </p>
      {#if dirty}
        <div class="flex shrink-0 items-center gap-2">
          {#if onDiscard}<Button variant="ghost" size="sm" disabled={saving} onclick={onDiscard}>Discard</Button>{/if}
          <Button size="sm" disabled={saving || disabled} onclick={onSave}>{label}</Button>
        </div>
      {/if}
    </div>
  </div>
{/if}
