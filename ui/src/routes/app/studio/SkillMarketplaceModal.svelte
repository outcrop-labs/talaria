<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ArrowLeft, Check, ExternalLink, Plus, Store } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage, getJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import {
    SKILLS_KEY,
    installMarketplaceSkills,
    marketplaceDetail,
    marketplaceKey,
    type InstallOutcome,
    type MarketplaceEntry,
    type MarketplaceSkill,
    type SkillOwner,
  } from '@/lib/skills'

  /** The skills marketplace, on the MCP marketplace's pattern: the ranked
   *  catalog searched live, one-click install when a repo is a single skill,
   *  a pick-list when it is a pack (every skill preselected — one click
   *  still installs the lot). The destination is an owner — one agent, or
   *  the shared root every agent carries. */
  let { owner, owners, onClose }: { owner: string; owners: SkillOwner[]; onClose: () => void } = $props()

  const qc = useQueryClient()
  const destinations = $derived(owners.filter((o) => o.canEdit))
  // From the PROP, not the derived: an initial value read once is the
  // point, and reading `owners` here keeps that obvious.
  const editable = owners.filter((o) => o.canEdit)
  let destination = $state(editable.some((o) => o.owner === owner) ? owner : (editable[0]?.owner ?? 'shared'))
  let q = $state('')
  let error = $state<string | null>(null)
  // The repo a click is resolving (fetching its skill list), keyed by repo.
  let busyRepo = $state<string | null>(null)
  // The pack opened for picking, with its discovered skills.
  let open = $state<{ entry: MarketplaceEntry; skills: MarketplaceSkill[] } | null>(null)
  let picking = $state<Set<string>>(new Set())
  // Per-repo install state, and per-skill statuses of the last install.
  let installed = $state<Map<string, true>>(new Map())
  let statuses = $state<Map<string, InstallOutcome['status']>>(new Map())

  const resultsQuery = createQuery(() => ({
    queryKey: marketplaceKey(q),
    queryFn: (): Promise<{ entries: MarketplaceEntry[] }> =>
      getJson<{ entries: MarketplaceEntry[] }>(
        q.trim() ? `/api/skills/marketplace?q=${encodeURIComponent(q.trim())}` : '/api/skills/marketplace',
      ),
    placeholderData: (prev: { entries: MarketplaceEntry[] } | undefined) => prev,
    staleTime: 60_000,
  }))
  const results = $derived(resultsQuery.data)

  // A destination's existing skills — the honest "already installed" check
  // for the common single-skill shape (repo name == skill name).
  const existingNames = $derived(new Set(owners.find((o) => o.owner === destination)?.skills.map((s) => s.name) ?? []))

  const repoDone = (e: MarketplaceEntry) => installed.has(e.repo) || existingNames.has(e.name)

  const runInstall = async (entry: MarketplaceEntry, skills?: string[]) => {
    busyRepo = entry.repo
    error = null
    try {
      const { results } = await installMarketplaceSkills(destination, entry.repo, skills)
      for (const r of results) statuses = new Map(statuses).set(`${entry.repo}/${r.name}`, r.status)
      installed = new Map(installed).set(entry.repo, true)
      open = null
      await qc.invalidateQueries({ queryKey: SKILLS_KEY })
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busyRepo = null
    }
  }

  /** One click: resolve the repo, then either install straight away (a
   *  single skill) or open the pick-list (a pack). */
  const install = async (entry: MarketplaceEntry) => {
    busyRepo = entry.repo
    error = null
    try {
      const { skills } = await marketplaceDetail(entry.repo)
      if (skills.length <= 1) {
        await runInstall(entry)
      } else {
        picking = new Set(skills.map((s) => s.name))
        statuses = new Map()
        open = { entry, skills }
      }
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busyRepo = entry.repo === busyRepo ? null : busyRepo
    }
  }

  const installPicked = () => {
    if (!open || busyRepo) return
    const names = open.skills.map((s) => s.name).filter((n) => picking.has(n))
    if (names.length) void runInstall(open.entry, names)
  }
</script>

<Modal open {onClose} title="Skills marketplace" takeover>
  <div class="flex h-full min-h-0 flex-col gap-4">
    {#if open}
      <!-- The pack picker: every skill the repo carries, preselected. -->
      <div class="flex items-center gap-3">
        <Button size="sm" variant="ghost" onclick={() => (open = null)}>
          <ArrowLeft size={14} />
          Browse
        </Button>
        <div class="min-w-0 flex-1">
          <div class="truncate font-sans text-sm font-semibold text-fg">{open.entry.repo}</div>
          <div class="truncate text-xs text-muted">{open.skills.length} skills in this pack</div>
        </div>
        <Button size="sm" onclick={installPicked} disabled={busyRepo === open.entry.repo || picking.size === 0}>
          {#if busyRepo === open.entry.repo}
            <WaitingMark site="skills/marketplace-install" size={12} />
          {/if}
          Install {picking.size || ''} to {destinations.find((o) => o.owner === destination)?.label ?? destination}
        </Button>
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
      <div class="min-h-0 flex-1 overflow-y-auto">
        <div class="space-y-2">
          {#each open.skills as s (s.name)}
            {@const status = statuses.get(`${open.entry.repo}/${s.name}`)}
            <label
              class={cn(
                'flex cursor-pointer items-start gap-3 rounded-lg border border-line-subtle p-3 transition-colors',
                status ? 'cursor-default opacity-80' : 'hover:border-line dither-fill',
              )}
            >
              <Checkbox
                bare
                checked={status ? status === 'exists' ? false : true : picking.has(s.name)}
                disabled={!!status}
                title={`Include ${s.name}`}
                onChange={(v) => {
                  const next = new Set(picking)
                  if (v) next.add(s.name)
                  else next.delete(s.name)
                  picking = next
                }}
              />
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-2">
                  <span class="truncate text-sm font-medium text-fg">{s.name}</span>
                  {#if status === 'installed'}
                    <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-success">installed</span>
                  {:else if status === 'exists'}
                    <span
                      class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted"
                      title="A skill with this name is already there — it was left exactly as it was"
                    >
                      already present
                    </span>
                  {/if}
                  <span class="shrink-0 font-mono text-[10px] text-ink-dim">{s.fileCount} files</span>
                </span>
                <span class="mt-0.5 block text-sm leading-snug text-muted">{s.summary || '…'}</span>
              </span>
            </label>
          {/each}
        </div>
      </div>
    {:else}
      <div class="flex items-center gap-3">
        <Input autofocus bind:value={q} placeholder="Search the catalog: superpowers, drawio, cybersecurity…" class="max-w-md" />
        {#if resultsQuery.isFetching}<WaitingMark site="skills/marketplace-search" size={12} class="text-muted" />{/if}
        <span class="flex-1"></span>
        <span class="flex items-center gap-1.5 text-xs text-muted">
          Install into
          <Select bind:value={destination} class="max-w-44 text-xs">
            {#each destinations as o (o.owner)}
              <option value={o.owner}>{o.owner === 'shared' ? 'Every agent' : o.label}</option>
            {/each}
          </Select>
        </span>
      </div>
      <div class="flex items-center gap-1.5">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Top skills</span>
        <InfoTip text="Hermes Atlas's ranking of the Hermes ecosystem's skill repos by GitHub stars. Installing copies the repo's SKILL.md directories into the destination — support files ride along, and the agent reads them live. Install only what you trust." />
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
      <div class="min-h-0 flex-1 overflow-y-auto">
        {#if resultsQuery.isError}
          <EmptyState
            icon="⚠"
            title="Marketplace didn't load"
            hint="The skills catalog could not be reached. Close and reopen the marketplace to retry."
          />
        {:else if !results}
          <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {#each Array.from({ length: 9 }, (_, i) => i) as i (i)}
              <div class="rounded-lg border border-line-subtle bg-surface p-4">
                <Skeleton class="h-3.5 w-32 rounded-full" />
                <Skeleton class="mt-2 h-2.5 w-24 rounded-full" />
                <Skeleton class="mt-3 h-2.5 w-full rounded-full" />
                <Skeleton class="mt-1.5 h-2.5 w-4/5 rounded-full" />
              </div>
            {/each}
          </div>
        {:else if results.entries.length === 0}
          <EmptyState
            icon="✦"
            title="No skills match"
            hint="The catalog ranks the Hermes ecosystem's skill repos. Try a broader search."
          />
        {:else}
          <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {#each results.entries as e (e.repo)}
              {@const done = repoDone(e)}
              {@const busy = busyRepo === e.repo}
              <div class="group relative flex flex-col rounded-lg border border-line-subtle p-4 transition-colors hover:border-line dither-fill">
                <div class="flex items-center gap-3">
                  <span class="grid h-[34px] w-[34px] shrink-0 place-items-center rounded-lg border border-line-subtle bg-surface text-muted">
                    <Store size={16} />
                  </span>
                  <div class="min-w-0 flex-1">
                    <div class="truncate font-sans text-sm font-semibold text-fg">{e.name}</div>
                    <div class="flex items-center gap-1.5">
                      <span class="truncate font-mono text-[11px] text-muted">{e.org}</span>
                      <span
                        title={`${e.repo} is #${e.rank} on Hermes Atlas's ranking`}
                        class="shrink-0 rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim"
                      >
                        ★ {e.stars}
                      </span>
                    </div>
                  </div>
                </div>
                <p class="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
                  {e.description || 'No description.'}
                </p>
                <a
                  href="https://github.com/{e.repo}"
                  target="_blank"
                  rel="noreferrer"
                  class="mt-1 inline-flex items-center gap-1 self-start font-mono text-[10px] text-muted opacity-0 transition-opacity hover:text-accent group-hover:opacity-100"
                >
                  <ExternalLink size={10} />
                  {e.repo}
                </a>
                <!-- The install control: quiet until the card is engaged. -->
                <div class="absolute right-3 top-3">
                  {#if done && !busy}
                    <span class="grid h-7 w-7 place-items-center rounded-full bg-success/10 text-success" title="Already in this destination">
                      <Check size={14} />
                    </span>
                  {:else}
                    <button
                      type="button"
                      title={busy ? 'Reading the repo' : `Install into ${destinations.find((o) => o.owner === destination)?.label ?? destination}`}
                      disabled={busy}
                      onclick={() => void install(e)}
                      class={cn(
                        'grid h-7 w-7 place-items-center rounded-full border transition-all',
                        busy
                          ? 'border-line bg-raised text-muted opacity-100'
                          : 'border-line bg-raised text-muted opacity-0 hover:border-accent hover:text-accent group-hover:opacity-100',
                      )}
                    >
                      {#if busy}<WaitingMark site="skills/marketplace-install" size={12} />{:else}<Plus size={14} />{/if}
                    </button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</Modal>
