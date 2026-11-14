<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SavedMark from '@/components/admin/SavedMark.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { permGroups, type PermsData } from './admin'

  /** Org-wide member defaults — what a plain member can do before any per-user
   *  override. Each is a switch; returning to the shipped default clears the row. */
  let { perms }: { perms: PermsData } = $props()

  const qc = useQueryClient()
  const autosave = useAutosave()
  const set = async (perm: string, enabled: boolean | null) => {
    try {
      await putJson<{ ok: true }>('/api/admin/permissions', { orgDefault: { perm, enabled } })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
    autosave.flash(perm)
  }
</script>

<SettingsSection
  title="Member permissions"
  description="What every member can do unless you change it for one person."
  info="Admins can always do all of these. To make an exception for one person, open their row in People."
>
  <!-- One card for all of them: a quiet row names each group, and its switches
       sit two to a row under it. -->
  <SettingsGroup>
    {#each permGroups(perms.catalog) as [group, entries] (group)}
      <div class="bg-raised/40 px-5 py-2 font-sans text-[13px] font-medium leading-5 text-fg">{group}</div>
      <div class="grid sm:grid-cols-2 [&>*+*]:border-t [&>*]:border-line-subtle sm:[&>*:nth-child(2)]:border-t-0 sm:[&>*:nth-child(odd)]:border-r sm:[&>*:last-child:nth-child(odd)]:col-span-2 sm:[&>*:last-child:nth-child(odd)]:border-r-0">
        {#each entries as p (p.id)}
          {@const effective = perms.orgDefaults[p.id] ?? p.memberDefault}
          <div class="flex min-h-[56px] items-center gap-2 px-5 py-3.5">
            <span class="min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg">{p.label}</span>
            <InfoTip text={p.hint} />
            {#if perms.orgDefaults[p.id] !== undefined && perms.orgDefaults[p.id] !== p.memberDefault}
              <Chip tone="warn" title="Changed from Talaria's own default">Changed</Chip>
            {/if}
            <SavedMark on={autosave.saved(p.id)} class="ml-auto pl-2" />
            <Toggle
              class="shrink-0 pl-1"
              title={p.label}
              checked={effective}
              onChange={() => void set(p.id, !effective === p.memberDefault ? null : !effective)}
            />
          </div>
        {/each}
      </div>
    {/each}
  </SettingsGroup>
</SettingsSection>
