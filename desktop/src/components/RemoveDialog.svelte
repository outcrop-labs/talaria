<script lang="ts">
  import { remove, shell } from '../lib/state.svelte'
  import Modal from './Modal.svelte'

  let { id }: { id: string } = $props()

  const label = $derived(shell.instances.find((i) => i.id === id)?.label ?? 'this instance')
  const cancel = () => (shell.confirmRemove = null)
  const confirm = () => {
    // `id` reads shell.confirmRemove through the prop: take it before closing.
    const target = id
    shell.confirmRemove = null
    void remove(target)
  }
</script>

<!-- Focus starts on Cancel: Enter on a freshly opened confirm must not delete. -->
<Modal label="Remove {label}" onescape={cancel} class="w-64">
  <div class="rounded-md border border-hairline bg-panel p-4">
    <p class="text-sm text-readout">Remove {label}?</p>
    <p class="mt-1 text-xs text-muted">
      Its saved session is deleted with it. The instance itself is untouched.
    </p>
    <div class="mt-4 flex justify-end gap-2">
      <button
        class="rounded px-3 py-1.5 text-xs text-muted hover:bg-hover hover:text-readout"
        data-autofocus
        onclick={cancel}
      >
        Cancel
      </button>
      <button
        class="rounded border border-danger px-3 py-1.5 font-mono text-xs text-danger hover:bg-danger/10"
        onclick={confirm}
      >
        REMOVE
      </button>
    </div>
  </div>
</Modal>
