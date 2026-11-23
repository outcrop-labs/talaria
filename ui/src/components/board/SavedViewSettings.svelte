<script lang="ts">
  import { popHeader } from '@/components/chat/chat-chrome'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import RenameField from '@/components/ui/RenameField.svelte'
  import SettingsButton from '@/components/ui/SettingsButton.svelte'

  // A saved view's settings, behind the gear beside its tab: its name, the
  // filters it saves, and Delete on the panel's danger line. The tab's
  // right-click menu offers the same three; this is the visible door, so the
  // view's settings are found without knowing to right-click, and Delete no
  // longer sits as a peer of the settings in one menu.
  //
  // `dirty` says the board no longer shows what the view saved — the person
  // changed a filter after applying it — and only then is there anything to
  // update. The writes are the board's (Board.svelte): this owns no request.
  let {
    viewId,
    name,
    dirty,
    onRename,
    onUpdate,
    onDelete,
  }: {
    viewId: string
    name: string
    dirty: boolean
    onRename: (name: string) => void
    onUpdate: () => void
    onDelete: () => void
  } = $props()
</script>

<Popover align="left" class="w-64">
  {#snippet trigger(open)}
    <SettingsButton label="View settings" active={open} aria-expanded={open} />
  {/snippet}
  {#snippet content(close)}
    <div class={popHeader}>View settings</div>
    <div class="space-y-3 px-2 pb-2 pt-1" data-view-settings>
      <label class="block">
        <span class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</span>
        <RenameField value={name} key={viewId} onCommit={onRename} class="w-full" />
      </label>
      <div>
        <Button
          variant="outline"
          size="sm"
          disabled={!dirty}
          onclick={() => {
            onUpdate()
            close()
          }}
        >
          Update to current filters
        </Button>
        <p class="mt-1 font-sans text-xs text-muted">
          {dirty ? 'Save the layout, grouping and filters the board shows now into this view.' : 'The board shows this view as saved.'}
        </p>
      </div>
      <div class="border-t border-line-subtle pt-2">
        <DangerLink
          onClick={() => {
            close()
            onDelete()
          }}
        >
          Delete view
        </DangerLink>
      </div>
    </div>
  {/snippet}
</Popover>
