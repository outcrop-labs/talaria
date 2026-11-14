<script lang="ts">
  import { Check } from '@lucide/svelte'
  import { provideAutosave } from '@/components/settings/autosave.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import type { TaskWorkflow } from '@/lib/workflows'
  import WorkflowDetail from './WorkflowDetail.svelte'

  // The dialog a Studio "Routed work" row opens. Every field in it saves as
  // it is left, so the footer says so where a Save would be and flashes Saved
  // there: the page's own cue is behind the dialog.
  let {
    workflow,
    onClose,
    onChanged,
    onDelete,
  }: {
    workflow: TaskWorkflow
    onClose: () => void
    onChanged: () => void
    /** Deletes the workflow. The confirmation is the caller's. */
    onDelete: () => void
  } = $props()

  const cue = provideAutosave()
</script>

<Modal open {onClose} width="max-w-2xl" title={workflow.name}>
  {#key workflow.id}
    <WorkflowDetail {workflow} {onChanged} />
  {/key}
  {#snippet footer()}
    <div class="flex items-center gap-4">
      <DangerLink onClick={onDelete}>Delete workflow</DangerLink>
      <span role="status" class="ml-auto flex items-center gap-1.5 whitespace-nowrap font-sans text-[13px] leading-5 text-muted">
        {#if cue.saved}<Check size={14} aria-hidden="true" class="text-success" />Saved{:else if cue.active}Changes save automatically{/if}
      </span>
      <Button variant="outline" size="sm" onclick={onClose}>Done</Button>
    </div>
  {/snippet}
</Modal>
