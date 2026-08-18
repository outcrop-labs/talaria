<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { MoreHorizontal, Lock } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { copySkillTo, deleteSkillReq, moveSkillTo, renameSkill, type SkillLibraryOwner } from '@/lib/workflows'

  let {
    owner,
    skill,
    owners,
    sharedBadge,
    canEdit,
    onOpen,
  }: {
    owner: string
    skill: { name: string; description: string; platform?: boolean }
    owners: SkillLibraryOwner[]
    sharedBadge?: boolean
    canEdit: boolean
    onOpen: () => void
  } = $props()

  const qc = useQueryClient()
  const refresh = () => qc.invalidateQueries({ queryKey: ['skill-library'] })
  const editableTargets = $derived(owners.filter((o) => o.canEdit && o.owner !== owner))
  const fail = (e: unknown) => void confirm({ title: 'That didn’t work', message: (e as Error).message, confirmLabel: 'OK' })

  const doRename = async () => {
    const to = await prompt({ title: 'Rename skill', message: 'Lowercase, dashes for spaces — agents load it by this name.', confirmLabel: 'Rename' })
    const name = to?.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name || name === skill.name) return
    await renameSkill(owner, skill.name, name).then(refresh).catch(fail)
  }
  const doDelete = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete "${skill.name}"? Workflows bound to it will flag it as missing.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteSkillReq(owner, skill.name).then(refresh).catch(fail)
  }
</script>

<div class="group flex w-full items-center gap-3 px-5 py-3 transition-colors hover:dither-fill">
  <button type="button" onclick={onOpen} class="min-w-0 flex-1 text-left">
    <span class="flex items-center gap-2">
      <span class="truncate text-sm font-medium text-fg">{skill.name}</span>
      {#if skill.platform}
        <Chip title="Platform skill — essential plumbing, admins only" class="shrink-0">
          <Lock size={9} class="mr-0.5 inline" />
          platform
        </Chip>
      {/if}
      {#if sharedBadge}<Chip class="shrink-0">shared</Chip>{/if}
    </span>
    <span class="mt-0.5 block text-sm leading-snug text-muted">{skill.description || '…'}</span>
  </button>
  {#if canEdit}
    <DropdownMenu
      items={[
        { label: 'Open', onSelect: onOpen },
        { label: 'Rename…', onSelect: () => void doRename() },
        ...(editableTargets.length
          ? [
              {
                label: 'Copy to',
                children: editableTargets.map((o) => ({
                  label: o.owner === 'shared' ? 'Every agent' : o.label,
                  onSelect: () => void copySkillTo(owner, skill.name, o.owner).then(refresh).catch(fail),
                })),
              },
            ]
          : []),
        ...(owner !== 'shared' && editableTargets.some((o) => o.owner === 'shared')
          ? [{ label: 'Promote to every agent', onSelect: () => void moveSkillTo(owner, skill.name, 'shared').then(refresh).catch(fail) }]
          : []),
        'sep' as const,
        { label: 'Delete', danger: true, onSelect: () => void doDelete() },
      ]}
    >
      {#snippet trigger(open)}
        <span
          class={cn(
            'rounded-md p-1 text-muted transition-opacity hover:text-fg',
            open ? 'opacity-100' : 'opacity-0 group-hover:opacity-100',
          )}
        >
          <MoreHorizontal size={16} />
        </span>
      {/snippet}
    </DropdownMenu>
  {/if}
</div>
