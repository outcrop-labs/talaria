<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Input from '@/components/ui/Input.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import RecordEditor from '@/components/editor/RecordEditor.svelte'
  import { confirmDelete } from '@/components/ui/confirm.svelte'
  import { SKILLS_KEY, deleteSkill, renameSkill, saveSkill, skillKey, skillName, useSkill } from '@/lib/skills'
  import { draftSkillForm } from '@/lib/muse.svelte'

  // ONE SKILL.md IN THE MODAL WORKSPACE — the Studio's deep-link overlay
  // (?sk=owner/name). The library views embed <RecordEditor> directly; this
  // is the same surface in a modal, because the Studio opens it over the
  // Studio. Everything the surface does — rename, workbench, whole-form Muse,
  // delete — is the record's, not the modal's.
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
  // The skill's live directory name: a save that renames moves it, and the
  // overlay follows rather than reading a path that no longer exists.
  let currentName = $state(name)
  const query = useSkill(() => owner, () => currentName)
  let busy = $state(false)
  let failure = $state<unknown>(null)
  let skillNameInput = $state(name)

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: SKILLS_KEY })
    await qc.invalidateQueries({ queryKey: skillKey(owner, currentName) })
    onChanged?.()
  }

  // The record's Save. A save that renames is two writes: move the directory
  // first, then write the content under the new name.
  const saveAll = async (content: string) => {
    busy = true
    failure = null
    try {
      const toName = skillName(skillNameInput) || currentName
      if (toName !== currentName) {
        await renameSkill(owner, currentName, toName)
        currentName = toName
      }
      await saveSkill(owner, toName, content)
      await refresh()
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }

  const remove = async () => {
    if (
      !(await confirmDelete({
        what: 'skill',
        name: currentName,
        detail: `Deleting “${currentName}” removes its whole directory. Anything bound to it will flag it as missing.`,
      }))
    )
      return
    try {
      await deleteSkill(owner, currentName)
      await refresh()
      onClose()
    } catch (e) {
      failure = e
    }
  }

  const museContext = $derived(
    owner === 'shared'
      ? 'A shared skill every agent on the team can use. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.'
      : `A skill for the "${ownerLabel}" agent. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.`,
  )
</script>

<!-- The editor seeds ONCE from the read — don't mount it until the content is
     here. A failed read must never seed it with '': saving from there would
     replace the real SKILL.md with an empty file. But the click has to land
     NOW, so the same shell shows with a skeleton body meanwhile. -->
{#if !query.data}
  <Modal open onClose={query.isError ? onClose : () => {}} width="max-w-6xl" title={`${currentName} · SKILL.md`}>
    {#if query.isError}
      <QueryError variant="compact" error={query.error} title={`Could not open ${currentName}`} onRetry={() => void query.refetch()} />
    {:else}
      <div class="space-y-3">
        <Skeleton class="h-2.5 w-2/3 rounded-full" />
        <Skeleton class="h-2.5 w-full rounded-full" />
        <Skeleton class="h-2.5 w-3/4 rounded-full" />
      </div>
    {/if}
  </Modal>
{:else}
  <!-- padded=false: the record surface owns its inset, so its pinned menu sits
       flush against the modal's frame like any other surface's. -->
  <Modal open onClose={onClose} width="max-w-6xl" title={`${currentName} · SKILL.md`} padded={false}>
    <div class="h-[76vh]">
      <RecordEditor
        kind="skill"
        title={currentName}
        meta={ownerLabel}
        subtitle="Read live; agents pick up edits on their next run."
        fieldsDirty={canEdit ? skillName(skillNameInput) !== currentName : false}
        onDelete={canEdit ? () => void remove() : undefined}
        doc={{
          value: query.data.content,
          editable: canEdit,
          saving: busy,
          onSave: canEdit ? saveAll : () => Promise.resolve(),
          history: { kind: 'skill', owner, name: currentName },
          // No doc.muse: the whole-form Muse drafts name AND content, so the
          // workbench carries no composer of its own.
        }}
        formMuse={
          canEdit
            ? {
                label: 'skill',
                current: (docText) => ({ name: skillName(skillNameInput) || currentName, content: docText }),
                draft: async (input, signal) => draftSkillForm({ ...input, context: museContext }, signal),
                fields: (d: { name?: string; content?: string; error?: string }) => [
                  { label: 'name', value: String(d.name ?? '').slice(0, 40) },
                ],
                docOf: (d: { content?: string }) => String(d.content ?? ''),
                apply: (d: { name?: string }) => {
                  const n = String(d.name ?? '').trim()
                  if (n) skillNameInput = n
                },
              }
            : undefined
        }
        onClose={onClose}
        onCancel={canEdit ? () => (skillNameInput = currentName) : undefined}
        class="h-full"
      >
        {#snippet fields(_)}
          {#if canEdit}
            <div class="mb-4 rounded-lg border border-line bg-card/40 p-3">
              <div class="mb-1.5 flex items-center gap-1.5">
                <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Skill name</span>
                <InfoTip text="The skill's directory name: lowercase letters, digits, dots, underscores and hyphens. Renaming is part of the record's Save." />
              </div>
              <Input size="sm" bind:value={skillNameInput} class="max-w-sm font-mono" />
            </div>
          {/if}
        {/snippet}
      </RecordEditor>
    </div>
  </Modal>
  <!-- A refused write used to be invisible: the old editors ignored the
       response, so a 403 closed cleanly and the edit was simply gone. -->
  {#if failure}
    <QueryError variant="inline" error={failure} title="Could not save this skill" />
  {/if}
{/if}
