<script lang="ts">
  import { Button, Chip, EmptyState, Input, SkeletonRows, useAppInvalidate } from '@talaria/sdk'
  import { fade, QUICK } from '@talaria/sdk'
  import { APP, useContacts, useStages, type ContactDoc } from './contacts'
  import ContactModal from './ContactModal.svelte'

  // ── Work surface ───────────────────────────────────────────────────────────
  let q = $state('')
  let stageFilter = $state<string | null>(null)
  let editing = $state<ContactDoc | 'new' | null>(null)
  const query = useContacts(() => q)
  const cfg = useStages()
  const invalidate = useAppInvalidate(APP)
  const stages = $derived(cfg.data?.stages ?? [])

  const contacts = $derived((query.data?.contacts ?? []).filter((c) => !stageFilter || c.data.stage === stageFilter))
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-3xl">
    <div class="mb-6 flex items-center gap-3">
      <h1 class="flex-1 font-sans text-lg font-semibold text-fg">Contacts</h1>
      <Input bind:value={q} placeholder="Search…" size="sm" class="w-48" />
      <Button size="sm" onclick={() => (editing = 'new')}>New contact</Button>
    </div>

    {#if stages.length > 0}
      <div class="mb-4 flex flex-wrap gap-1.5">
        {#each [null, ...stages] as s (s ?? 'all')}
          <Chip onSelect={() => (stageFilter = s)} selected={stageFilter === s} class="capitalize">
            {s ?? 'all'}
          </Chip>
        {/each}
      </div>
    {/if}

    {#if query.isLoading}
      <SkeletonRows rows={5} />
    {:else if contacts.length === 0}
      <div in:fade={{ duration: 150 }}>
        <EmptyState
          icon="☏"
          title={q || stageFilter ? 'No matches' : 'No contacts yet'}
          hint={q || stageFilter ? undefined : 'Add your first contact to start the pipeline'}
        />
      </div>
    {:else}
      <ul class="divide-y divide-line overflow-hidden rounded-lg border border-line bg-panel">
        {#each contacts as c (c.id)}
          <li
            in:fade={{ duration: 150 }}
            out:fade={QUICK}
            onclick={() => (editing = c)}
            class="flex cursor-pointer items-center gap-3 px-4 py-3 transition-colors duration-120 hover:bg-hover"
          >
            <div class="min-w-0 flex-1">
              <div class="truncate text-sm font-medium text-fg">{c.data.name}</div>
              <div class="truncate text-xs text-muted">
                {[c.data.company, c.data.email].filter(Boolean).join(' · ') || '—'}
              </div>
            </div>
            {#if c.data.stage}<Chip class="capitalize">{c.data.stage}</Chip>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  {#if editing}
    <ContactModal
      doc={editing === 'new' ? null : editing}
      {stages}
      onClose={() => (editing = null)}
      onSaved={() => {
        editing = null
        void invalidate()
      }}
    />
  {/if}
</div>
