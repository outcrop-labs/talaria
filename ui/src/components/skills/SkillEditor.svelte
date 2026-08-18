<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { SKILLS_KEY, deleteSkill, saveSkill, skillKey, useSkill } from '@/lib/skills'

  // ONE SKILL.md IN THE FULL WORKSPACE EDITOR — rich editing, Muse drafting,
  // version history. Reads live; agents pick up saves on their next run.
  //
  // This replaces three near-identical editors (assistant, fleet, Studio) that
  // differed only in query keys, delete wording, and whether they sent
  // credentials. They also all ignored the SAVE RESPONSE — a bare `await
  // fetch(...)` with no status check, so a 403 from `canEditSkill` reported
  // success and the editor closed as though the write had landed. Going
  // through `saveSkill`/`deleteSkill` means a refused write now throws and is
  // shown.
  let {
    owner,
    ownerLabel,
    name,
    canEdit = true,
    onClose,
    onChanged,
  }: {
    owner: string
    /** Whose library this is, for the subtitle and the Muse context. */
    ownerLabel: string
    name: string
    canEdit?: boolean
    onClose: () => void
    /** Extra invalidation for a caller with its own list key. */
    onChanged?: () => void
  } = $props()

  const qc = useQueryClient()
  const query = useSkill(() => owner, () => name)
  let busy = $state(false)
  let failure = $state<unknown>(null)

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: SKILLS_KEY })
    await qc.invalidateQueries({ queryKey: skillKey(owner, name) })
    onChanged?.()
  }

  const save = async (content: string) => {
    busy = true
    failure = null
    try {
      await saveSkill(owner, name, content)
      await refresh()
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }

  const remove = async () => {
    if (
      !(await confirm({
        title: 'Delete skill',
        message: `Delete "${name}"? Anything bound to it will flag it as missing.`,
        confirmLabel: 'Delete',
        danger: true,
      }))
    )
      return
    try {
      await deleteSkill(owner, name)
      await refresh()
      onClose()
    } catch (e) {
      failure = e
    }
  }

  const museContext = $derived(
    owner === 'shared'
      ? 'A shared skill available to every agent in the fleet. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.'
      : `A skill for the "${ownerLabel}" agent. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.`,
  )
</script>

<!-- The editor seeds ONCE from `value` — don't mount it until the content is
     here. A failed read must never seed it with '': saving from there would
     replace the real SKILL.md with an empty file. But the click has to land
     NOW, so the same shell shows with a prose-bar body meanwhile. -->
{#if !query.data}
  <!-- The Modal primitive carries the shell (backdrop, Escape, portal). While
       loading, dismissal stays inert; only a failed read may be Esc'd away. -->
  <Modal open onClose={query.isError ? onClose : () => {}} width="max-w-3xl">
    {#if query.isError}
      <QueryError
        variant="compact"
        error={query.error}
        title={`Could not open ${name}`}
        onRetry={() => void query.refetch()}
      />
    {:else}
      <div class="space-y-3">
        <Skeleton class="h-2.5 w-2/3 rounded-full" />
        <Skeleton class="h-2.5 w-full rounded-full" />
        <Skeleton class="h-2.5 w-3/4 rounded-full" />
      </div>
    {/if}
  </Modal>
{:else}
  {#snippet deleteAction()}
    <Button variant="ghost" size="sm" onclick={() => void remove()}>Delete skill</Button>
  {/snippet}
  <InternalEditorModal
    open
    {onClose}
    title={`${name} · SKILL.md`}
    subtitle={`${ownerLabel} — read live; agents pick up edits on their next run.`}
    value={query.data.content}
    editable={canEdit}
    saving={busy}
    onSave={save}
    history={{ kind: 'skill', owner, name }}
    muse={{ kind: 'skill', context: museContext }}
    footerExtra={canEdit ? deleteAction : undefined}
  />
  <!-- A refused write used to be invisible: the old editors ignored the
       response, so a 403 closed cleanly and the edit was simply gone. -->
  {#if failure}
    <QueryError variant="inline" error={failure} title="Could not save this skill" />
  {/if}
{/if}
