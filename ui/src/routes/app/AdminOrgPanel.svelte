<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { useAdminSettings } from './admin'

  // App-wide settings (grows over time). Audit retention is the first.
  // The business every agent works for. Woven automatically into agent design
  // (muse-generated souls anchor to this team) and every rendered SOUL.md — so
  // no agent introduces itself as belonging to the underlying platform.
  const qc = useQueryClient()
  const query = useAdminSettings()
  const data = $derived(query.data)
  let name = $state<string | null>(null)
  let about = $state<string | null>(null)
  const savedFlash = useSavedFlash()
  const nameVal = $derived(name ?? data?.org.name ?? '')
  const aboutVal = $derived(about ?? data?.org.about ?? '')
  const dirty = $derived(nameVal !== (data?.org.name ?? '') || aboutVal !== (data?.org.about ?? ''))

  const save = async () => {
    await fetch('/api/admin/settings', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ org: { name: nameVal, about: aboutVal } }),
    })
    name = null
    about = null
    savedFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }
</script>

<Panel>
  <SectionHeader class="mb-1" title="Organization" />
  <p class="mb-3 text-xs text-muted">
    The business your agents work for. Baked into every agent's identity automatically. Generated souls anchor to
    this team, and saving here rolls running agents (a fresh container comes up and traffic cuts over only once
    it's healthy), so the fleet speaks the new identity without interrupting anyone's conversation.
  </p>
  {#if query.isPending}
    <!-- Hold the form's footprint until settings land, so the fields never
         render blank and then fill in. -->
    <div class="space-y-3">
      <div>
        <Skeleton class="mb-1.5 h-2.5 w-24 rounded-full" />
        <Skeleton class="h-9 w-full max-w-xs" />
      </div>
      <div>
        <Skeleton class="mb-1.5 h-2.5 w-40 rounded-full" delay={0.12} />
        <Skeleton class="h-14 w-full" delay={0.12} />
      </div>
    </div>
  {:else if !data}
    <!-- A failed read used to render the identity form BLANK. Typing one word
         into it made it dirty, and Save then wrote an empty business name and
         description over the real ones — and rolled the whole fleet onto them. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your organization"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="space-y-3">
      <div>
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Business name</label>
        <Input size="sm" value={nameVal} oninput={(e) => (name = e.currentTarget.value)} placeholder="e.g. Outcrop Labs" class="max-w-xs" />
      </div>
      <div>
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">What the business does</label>
        <Textarea
          rows={2}
          value={aboutVal}
          oninput={(e) => (about = e.currentTarget.value)}
          placeholder="One or two sentences agents can anchor their mission to."
          class="w-full text-sm"
        />
      </div>
      <div class="flex items-center gap-2">
        {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
        <span class="ml-auto"></span>
        <Button size="sm" onclick={() => void save()} disabled={!dirty}>
          Save
        </Button>
      </div>
    </div>
  {/if}
</Panel>
