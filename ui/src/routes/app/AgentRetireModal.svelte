<script lang="ts">
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
  <div class="space-y-4">
    <p class="text-sm text-muted">
      The running container is removed and <span class="text-fg">{d.displayName}</span> leaves the fleet. Its state
      volume (memories, plans) and version history are kept, so it can be brought back later.
    </p>
    <div>
      <label class="mb-1 block text-xs text-muted">
        Type <code class="text-fg">{d.slug}</code> to confirm
      </label>
      <Input bind:value={typed} placeholder={d.slug} autofocus />
    </div>
    <div class="flex justify-end gap-2 border-t border-line pt-3">
      <Button variant="ghost" size="sm" onclick={onClose}>
        Cancel
      </Button>
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
  </div>
</Modal>
