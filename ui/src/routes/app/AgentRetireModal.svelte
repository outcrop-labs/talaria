<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import type { AgentDef } from '@/lib/fleet-defs'

  // Retiring removes the container and drops the agent from the fleet — a
  // destructive action, so it's a double opt-in: type the agent's slug to confirm.
  let { def: d, onClose, onConfirm }: { def: AgentDef; onClose: () => void; onConfirm: () => void } = $props()

  let typed = $state('')
  const match = $derived(typed.trim() === d.slug)
</script>

<Modal open {onClose} title={`Retire ${d.displayName}?`} width="max-w-md">
  <div class="space-y-5">
    <p class="font-sans text-[13px] leading-5 text-muted">
      Its container is removed and <span class="text-fg">{d.displayName}</span> leaves the team. Its memory and version
      history are kept, so it can be brought back later.
    </p>
    <SettingsField label="Type its handle to confirm" for="retire-confirm" hint={`The handle is ${d.slug}.`}>
      <Input id="retire-confirm" size="sm" class="font-mono" bind:value={typed} placeholder={d.slug} autofocus />
    </SettingsField>
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
      <Button
        size="sm"
        variant="danger"
        disabled={!match}
        onclick={() => {
          onConfirm()
          onClose()
        }}
      >
        Retire agent
      </Button>
    </div>
  {/snippet}
</Modal>
