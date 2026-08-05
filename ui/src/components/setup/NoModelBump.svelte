<script lang="ts" module>
  // Bumps: the things this instance still needs, said where they actually bite.
  //
  // There is no setup wizard on purpose. A gate before the product replaces one
  // wall with a nicer wall, and the wall is the problem — a new operator's first
  // act is entering a provider key, which is itself a secret write, so a broken
  // secretbox used to make the app unusable behind a message about a root secret
  // they had never heard of.
  //
  // So: nothing blocks. A missing provider key stops you CHATTING, not using the
  // app, and the fix is offered in the same place the gap appears.
  //
  // Every bump (this, UnreadableSecretsBanner, NoEmailBump) follows two rules.
  // It appears only on a RESOLVED gap — a failed read is never rendered as "you
  // have not configured this", because accusing an operator of a gap they do not
  // have sends them to fix something that was never broken. And it says what the
  // gap costs, not just that it exists.

  // ── No model configured ──────────────────────────────────────────────────────

  import { PROVIDER_PRESETS } from '@/lib/models'

  const KEYED_PRESETS = PROVIDER_PRESETS.filter((p) => p.class === 'cloud' && p.apiKeyEnv)
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { useSession } from '@/lib/session'
  import { useModels } from '@/lib/muse.svelte'
  import { addEndpoint, patchEndpoint } from '@/lib/models'
  import { getJson } from '@/lib/fetch-json'
  import { cn } from '@/lib/cn'
  import { p } from '@/router'

  // Shown on the surfaces that cannot work without a model. Admins get the
  // field right here; everyone else gets told who to ask, because they cannot
  // fix it and a form they cannot submit is worse than a sentence.
  let { class: className }: { class?: string } = $props()

  const session = useSession()
  const models = useModels()
  const qc = useQueryClient()
  let preset = $state(KEYED_PRESETS[0]?.key ?? '')
  let key = $state('')
  let busy = $state(false)
  let msg = $state<string | null>(null)

  const isAdmin = $derived(session.data?.role === 'admin')

  const add = async () => {
    const p = KEYED_PRESETS.find((x) => x.key === preset)
    if (!p || !key.trim()) return
    busy = true
    msg = null
    try {
      const created = await addEndpoint({
        name: p.key,
        provider: p.provider,
        baseUrl: p.baseUrl ?? null,
        class: p.class,
        apiKeyEnv: p.apiKeyEnv ?? null,
        apiKey: key.trim(),
      })
      if (created.error || !created.id) {
        msg = created.error ?? 'could not add that provider'
        return
      }
      key = ''
      // An endpoint with no models registered serves nothing, so adding a key
      // alone would leave the picker just as empty and the bump would look
      // broken. Register what the provider actually reports — live, never a
      // baked-in list — and say how many, with where to curate them.
      const avail = await getJson<{ models: string[]; note?: string }>(
        `/api/fleet/endpoints/${created.id}/available`,
      ).catch(() => ({ models: [] as string[], note: 'could not reach the provider' }))
      if (avail.models.length) {
        const patched = await patchEndpoint(created.id, { models: avail.models })
        msg = patched.error
          ? `Added ${p.label}, but its models could not be registered: ${patched.error}`
          : `${p.label} added with ${avail.models.length} model${avail.models.length === 1 ? '' : 's'}. Curate them on Models.`
      } else {
        msg = `${p.label} added, but it returned no models${avail.note ? ` — ${avail.note}` : ''}. Check the key on Models.`
      }
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['gateway-models'] }),
        qc.invalidateQueries({ queryKey: ['fleet-endpoints'] }),
      ])
    } finally {
      busy = false
    }
  }
</script>

<!-- A resolved empty catalog is the gap. Pending is not, and neither is a
     failed read — /api/models answering 500 must not read as "no models". -->
{#if !models.isPending && !models.isError && (models.data?.models.length ?? 0) === 0}
  <div class={cn('rounded-md border border-line bg-raised/40 p-4', className)}>
    <p class="mb-1 text-sm text-fg">No model is configured yet.</p>
    <p class="mb-3 text-xs leading-relaxed text-muted">
      {isAdmin
        ? 'Chat, Plan and Research all route through a provider. Add a key and this instance can answer — everything else keeps working either way.'
        : 'Chat, Plan and Research need a model provider. An admin can add one on Models; the rest of Talaria works without it.'}
    </p>
    {#if isAdmin}
      <div class="flex flex-wrap items-end gap-2">
        <Select bind:value={preset} class="w-40">
          {#each KEYED_PRESETS as prov (prov.key)}
            <option value={prov.key}>
              {prov.label}
            </option>
          {/each}
        </Select>
        <Input
          type="password"
          bind:value={key}
          placeholder="paste the API key"
          autocomplete="off"
          class="min-w-[14rem] flex-1"
        />
        <Button size="sm" onclick={() => void add()} disabled={busy || !key.trim()}>
          {busy ? 'Adding' : 'Add'}
        </Button>
      </div>
      <p class="mt-2 text-xs text-muted">
        {#if msg}
          {msg}
        {:else}
          Stored encrypted, never shown again. More providers and per-model curation live on
          <a href={p('/models')} class="underline underline-offset-2 hover:text-fg">Models</a>.
        {/if}
      </p>
    {/if}
  </div>
{/if}
