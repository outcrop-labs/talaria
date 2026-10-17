<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import AddRow from '@/components/ui/AddRow.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import {
    addDomain,
    featuredChips,
    isBareHost,
    saveFeatured,
    toggleChip,
    useMcpFeatured,
    type FeaturedConfigRow,
    type StandInRow,
  } from './mcp'

  /** The operator's say over the marketplace's featured shelf: the built-in
   *  publishers (hide the ones this org will never use), domains added on
   *  top, and stand-ins — "this publisher's server is at this URL" — for the
   *  companies the registry does not list or lists wrong. Admin only; the
   *  shelf re-reads when this closes. */
  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  const featuredQuery = useMcpFeatured()
  let draft = $state<FeaturedConfigRow | null>(null)
  let addError = $state<string | null>(null)
  let error = $state<string | null>(null)
  let saving = $state(false)

  // Seed the draft once from the first answer; later refetches must not
  // overwrite what the operator is mid-way through typing.
  $effect(() => {
    const d = featuredQuery.data
    if (d && !draft) {
      draft = {
        domains: [...d.domains],
        hidden: [...d.hidden],
        standIns: d.standIns.map((s) => ({ ...s, description: s.description ?? '' })),
      }
    }
  })

  const close = () => {
    // The shelf and the status line both read the merged list.
    void qc.invalidateQueries({ queryKey: ['mcp-library'] })
    onClose()
  }

  const onAdd = (raw: string) => {
    if (!draft) return
    if (!isBareHost(raw)) {
      addError = `“${raw}” is not a bare host name — write it like linear.app, without https:// or a path.`
      return
    }
    addError = null
    draft = addDomain(draft, raw)
  }

  const blankStandIn = (): StandInRow => ({ domain: '', title: '', url: '', description: '' })

  const standInProblem = (s: StandInRow): string | null => {
    if (!s.domain.trim() && !s.title.trim() && !s.url.trim()) return null
    if (!isBareHost(s.domain)) return 'Domain must be a bare host, like deepwiki.com.'
    if (!s.title.trim()) return 'Give it a title.'
    if (!/^https:\/\/[^\s{}]+$/i.test(s.url.trim())) return 'The endpoint must be an https:// URL.'
    return null
  }

  const save = async () => {
    if (!draft) return
    // Blank rows are the editor's own scaffolding, not a stand-in.
    const standIns = draft.standIns
      .filter((s) => s.domain.trim() || s.title.trim() || s.url.trim())
      .map((s) => ({
        domain: s.domain.trim(),
        title: s.title.trim(),
        url: s.url.trim(),
        description: s.description?.trim() ? s.description.trim() : null,
      }))
    const bad = standIns.map(standInProblem).find((p) => p !== null)
    if (bad) {
      error = bad
      return
    }
    saving = true
    error = null
    try {
      await saveFeatured({ ...draft, standIns })
      await qc.invalidateQueries({ queryKey: ['mcp-library-featured'] })
      close()
    } catch (e) {
      error = errorMessage(e)
    } finally {
      saving = false
    }
  }
</script>

<Modal open onClose={close} title="Featured servers" width="max-w-2xl">
  <QueryState query={featuredQuery} errorTitle="Couldn't load the featured list" errorVariant="compact">
    {#snippet skeleton()}
      <div class="flex flex-wrap gap-1.5">
        {#each Array.from({ length: 12 }, (_, i) => i) as i (i)}<Skeleton class="h-4 w-20 rounded" />{/each}
      </div>
    {/snippet}
    {#snippet children(data)}
      {#if draft}
        {@const chips = featuredChips(data.builtins, draft)}
        <div class="flex flex-col gap-5">
          <section class="flex flex-col gap-2">
            <div class="flex items-center gap-1.5">
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Publishers on the shelf</span>
              <InfoTip
                text="Each domain shows its publisher's own server — the first-party registry entry, or the stand-in below. Built-ins can be hidden and put back; added domains are removed. A publisher with no server anywhere simply doesn't appear."
              />
            </div>
            <div class="flex flex-wrap gap-1.5">
              {#each chips as c (c.domain)}
                {#if c.hidden}
                  <Chip
                    title="Hidden from the shelf — click to put it back"
                    onSelect={() => draft && (draft = toggleChip(data.builtins, draft, c.domain))}
                    class="normal-case line-through opacity-60"
                  >
                    {c.domain}
                  </Chip>
                {:else}
                  <Chip
                    tone={c.builtin ? 'neutral' : 'accent'}
                    title={c.builtin ? 'Built in — remove to hide it from the shelf' : 'Added by your org'}
                    onRemove={() => draft && (draft = toggleChip(data.builtins, draft, c.domain))}
                    class="normal-case"
                  >
                    {c.domain}{#if c.builtin}<span class="ml-1 text-ink-dim">· built-in</span>{/if}
                  </Chip>
                {/if}
              {/each}
            </div>
            <AddRow placeholder="Add a publisher's domain, e.g. deepwiki.com" onSubmit={onAdd} />
            {#if addError}<p class="text-xs text-danger">{addError}</p>{/if}
          </section>

          <section class="flex flex-col gap-2">
            <div class="flex items-center gap-1.5">
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Stand-ins</span>
              <InfoTip
                text="Your answer to “which endpoint is this publisher's server?” It features the domain by itself and wins over the registry's entry. New stand-ins are checked for liveness on the next batch; an unreachable one leaves the shelf."
              />
            </div>
            {#each draft.standIns as s, i (i)}
              {@const problem = standInProblem(s)}
              <div class="flex flex-col gap-1.5 rounded-md border border-line-subtle p-2.5">
                <div class="grid gap-1.5 sm:grid-cols-[1fr_1fr_2fr]">
                  <Input size="sm" bind:value={s.domain} placeholder="deepwiki.com" class="font-[var(--font-mono)]" aria-label="Domain" />
                  <Input size="sm" bind:value={s.title} placeholder="DeepWiki" aria-label="Title" />
                  <Input size="sm" bind:value={s.url} placeholder="https://mcp.deepwiki.com/mcp" class="font-[var(--font-mono)]" aria-label="Endpoint URL" />
                </div>
                <div class="flex items-center gap-2">
                  <Input size="sm" bind:value={s.description} placeholder="Description (optional)" class="flex-1" aria-label="Description" />
                  <DangerLink onClick={() => draft && (draft = { ...draft, standIns: draft.standIns.filter((_, j) => j !== i) })}>Remove</DangerLink>
                </div>
                {#if problem}<p class="text-xs text-warning">{problem}</p>{/if}
              </div>
            {/each}
            <div>
              <Button size="xs" variant="outline" onclick={() => draft && (draft = { ...draft, standIns: [...draft.standIns, blankStandIn()] })}>
                Add a stand-in
              </Button>
            </div>
          </section>

          {#if error}<p class="text-sm text-danger">{error}</p>{/if}
        </div>
      {/if}
    {/snippet}
  </QueryState>
  {#snippet footer()}
    <Button variant="ghost" onclick={close}>Cancel</Button>
    <Button disabled={!draft || saving} onclick={() => void save()} class="gap-1.5">
      {#if saving}<WaitingMark site="mcp/featured-save" size={12} />{/if}
      Save
    </Button>
  {/snippet}
</Modal>
