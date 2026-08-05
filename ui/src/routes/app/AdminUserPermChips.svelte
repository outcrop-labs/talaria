<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import AdminPermChip from './AdminPermChip.svelte'
  import type { PermCatalogEntry, PermsData } from './admin'

  /** Per-user fine-grained permissions: chips grouped by concern, showing the
   *  EFFECTIVE state (override → org default → shipped default). */
  let { userId, perms }: { userId: string; perms: PermsData } = $props()

  const qc = useQueryClient()
  const overrides = $derived(perms.overrides[userId] ?? {})
  const orgDefault = (p: PermCatalogEntry) => perms.orgDefaults[p.id] ?? p.memberDefault
  const set = async (perm: string, allowed: boolean | null) => {
    await fetch('/api/admin/permissions', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ userId, perm, allowed }),
    })
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
  }
</script>

<div class="flex flex-wrap items-center gap-1.5 pl-10">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">can</span>
  {#each perms.catalog as p (p.id)}
    {@const effective = overrides[p.id] ?? orgDefault(p)}
    <AdminPermChip
      entry={p}
      {effective}
      overridden={overrides[p.id] !== undefined}
      onToggle={() => void set(p.id, !effective === orgDefault(p) ? null : !effective)}
    />
  {/each}
</div>
