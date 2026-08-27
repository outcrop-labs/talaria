<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import AdminPermChip from './AdminPermChip.svelte'
  import { permGroups, type PermsData } from './admin'

  /** Org-wide member defaults — what a plain member can do before any per-user
   *  override. Chips toggle; returning to the shipped default clears the row. */
  let { perms }: { perms: PermsData } = $props()

  const qc = useQueryClient()
  const set = async (perm: string, enabled: boolean | null) => {
    try {
      await putJson<{ ok: true }>('/api/admin/permissions', { orgDefault: { perm, enabled } })
    } catch (e) {
      pushToast({ title: 'Save failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
  }
</script>

<Panel>
  <SectionHeader
    title="Member defaults"
    info="What every plain member may do out of the box. Per-person exceptions live on each row below; admins always hold every permission. A dot marks a default you've changed from Talaria's shipped baseline."
  />
  <div class="space-y-2">
    {#each permGroups(perms.catalog) as [group, entries] (group)}
      <div class="flex flex-wrap items-center gap-1.5">
        <span class="w-16 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{group}</span>
        {#each entries as p (p.id)}
          {@const effective = perms.orgDefaults[p.id] ?? p.memberDefault}
          <AdminPermChip
            entry={p}
            {effective}
            overridden={perms.orgDefaults[p.id] !== undefined && perms.orgDefaults[p.id] !== p.memberDefault}
            onToggle={() => void set(p.id, !effective === p.memberDefault ? null : !effective)}
          />
        {/each}
      </div>
    {/each}
  </div>
</Panel>
