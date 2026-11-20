<script lang="ts">
  import { popHeader } from '@/components/chat/chat-chrome'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import RenameField from '@/components/ui/RenameField.svelte'
  import SettingsButton from '@/components/ui/SettingsButton.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'

  // The focused workchain's settings, behind the gear beside its switcher:
  // its name, whether it is paused, and Delete on the panel's danger line.
  // The switcher keeps what it is for — switch, create, copy a link,
  // straighten — so Delete is no longer a peer of the settings in one menu.
  // The writes are the lens's (Workchains.svelte): this owns no request.
  let {
    chainId,
    name,
    paused,
    onRename,
    onPaused,
    onDelete,
  }: {
    chainId: string
    name: string
    paused: boolean
    onRename: (name: string) => void
    onPaused: (paused: boolean) => void
    onDelete: () => void
  } = $props()
</script>

<Popover align="left" class="w-64">
  {#snippet trigger(open)}
    <SettingsButton label="Workchain settings" active={open} aria-expanded={open} />
  {/snippet}
  {#snippet content(close)}
    <div class={popHeader}>Workchain settings</div>
    <div class="space-y-3 px-2 pb-2 pt-1" data-workchain-settings>
      <label class="block">
        <span class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</span>
        <RenameField value={name} key={chainId} onCommit={onRename} class="w-full" />
      </label>
      <div>
        <Toggle checked={paused} onChange={onPaused} label="Paused" />
        <p class="mt-1 font-sans text-xs text-muted">While paused, no step hands its work to the next.</p>
      </div>
      <div class="border-t border-line-subtle pt-2">
        <DangerLink
          onClick={() => {
            close()
            onDelete()
          }}
        >
          Delete workchain
        </DangerLink>
      </div>
    </div>
  {/snippet}
</Popover>
