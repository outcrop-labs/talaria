<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ArrowLeft, Check, ExternalLink, Plus, ShieldAlert, Store } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { ChipTone } from '@/components/ui/chip'
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
    installMarketplaceSkill,
    marketplaceDetail,
    marketplaceKey,
    type InstallOutcome,
    type MarketplaceEntry,
    type MarketplaceSkill,
    type ScanFinding,
    type SkillOwner,
  } from '@/lib/skills'

  /** The skills marketplace, on the MCP marketplace's pattern: Hermes'
   *  official skills index searched from the local mirror, one click per
   *  skill. The click fetches the skill and runs Hermes' own Skills Guard
   *  over it; a skill Hermes would install installs straight away, and one
   *  it would block opens the scan report instead — nothing is written. The
   *  destination is an owner — one agent, or the shared root every agent
   *  carries. */
  let { owner, owners, onClose }: { owner: string; owners: SkillOwner[]; onClose: () => void } = $props()

  const qc = useQueryClient()
  const destinations = $derived(owners.filter((o) => o.canEdit))
  // From the PROP, not the derived: an initial value read once is the
  // point, and reading `owners` here keeps that obvious.
  // svelte-ignore state_referenced_locally -- reason: initial-value read once is the point (the derived `destinations` tracks the live list); see the note above
  const editable = owners.filter((o) => o.canEdit)
  // svelte-ignore state_referenced_locally -- reason: destination draft seeded once at mount; the modal mounts per open
  let destination = $state(editable.some((o) => o.owner === owner) ? owner : (editable[0]?.owner ?? 'shared'))
  let q = $state('')
  let error = $state<string | null>(null)
  // The skill a click is resolving (fetch + scan + install), by catalog id.
  let busyId = $state<string | null>(null)
  // A skill the scan blocked, opened for reading: the entry and its preview.
  let blocked = $state<{ entry: MarketplaceEntry; skill: MarketplaceSkill } | null>(null)
  // Per-skill outcomes of this session's installs, by catalog id.
  let outcomes = $state<Map<string, InstallOutcome['status']>>(new Map())

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
  // for the common shape (the skill's directory is its catalog name).
  const existingNames = $derived(new Set(owners.find((o) => o.owner === destination)?.skills.map((s) => s.name) ?? []))
  const isDone = (e: MarketplaceEntry) => outcomes.has(e.id) || existingNames.has(e.name.toLowerCase())
  const destinationLabel = $derived(destinations.find((o) => o.owner === destination)?.label ?? destination)

  const TRUST_TONE: Record<string, ChipTone> = { builtin: 'accent', trusted: 'success', community: 'neutral' }
  const TRUST_NOTE: Record<string, string> = {
    builtin: 'Official — maintained by Nous Research with Hermes itself',
    trusted: 'From a repo Hermes trusts (OpenAI, Anthropic, Hugging Face, NVIDIA): installs unless the scan finds something critical',
    community: 'Community — installs only when the scan finds nothing high-risk',
  }
  const VERDICT_TONE: Record<string, ChipTone> = { safe: 'success', caution: 'warn', dangerous: 'danger' }
  const SEVERITY_TONE: Record<string, ChipTone> = { critical: 'danger', high: 'warn', medium: 'neutral', low: 'neutral' }
  // Worst first, as Hermes' own scan report orders them.
  const SEVERITY_RANK: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 }
  const worstFirst = (findings: ScanFinding[]) =>
    [...findings].sort((a, b) => (SEVERITY_RANK[a.severity] ?? 4) - (SEVERITY_RANK[b.severity] ?? 4))
  const githubUrl = (e: MarketplaceEntry) => `https://github.com/${e.repo}${e.path ? `/tree/HEAD/${e.path}` : ''}`

  /** One click: fetch and scan, then install what the scan allows; open the
   *  report for what it blocks. */
  const install = async (entry: MarketplaceEntry) => {
    busyId = entry.id
    error = null
    try {
      const skill = await marketplaceDetail(destination, entry.id)
      if (skill.scan.allowed !== true) {
        blocked = { entry, skill }
        return
      }
      const outcome = await installMarketplaceSkill(destination, entry.id)
      outcomes = new Map(outcomes).set(entry.id, outcome.status)
      await qc.invalidateQueries({ queryKey: SKILLS_KEY })
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busyId = null
    }
  }
</script>

<Modal open {onClose} title="Skills marketplace" takeover>
  <div class="flex h-full min-h-0 flex-col gap-4">
    {#if blocked}
      {@const scan = blocked.skill.scan}
      <!-- The scan report for a skill Hermes' install policy blocked. -->
      <div class="flex items-center gap-3">
        <Button size="sm" variant="ghost" onclick={() => (blocked = null)}>
          <ArrowLeft size={14} />
          Browse
        </Button>
        <div class="min-w-0 flex-1">
          <div class="truncate font-sans text-sm font-semibold text-fg">{blocked.entry.name}</div>
          <div class="truncate font-mono text-[11px] text-muted">{blocked.skill.upstream ?? blocked.entry.repo}</div>
        </div>
      </div>
      <div class="rounded-lg border border-danger/40 p-4">
        <div class="flex items-center gap-2">
          <ShieldAlert size={16} class="text-danger" />
          <span class="font-sans text-sm font-semibold text-fg">Not installed</span>
          <Chip tone={VERDICT_TONE[scan.verdict] ?? 'neutral'}>{scan.verdict}</Chip>
          <Chip tone={TRUST_TONE[scan.trustLevel] ?? 'neutral'} title={TRUST_NOTE[scan.trustLevel]}>{scan.trustLevel}</Chip>
        </div>
        <p class="mt-2 font-sans text-sm leading-relaxed text-muted">
          Hermes' Skills Guard ({scan.scanner}) found {scan.findings.length}
          {scan.findings.length === 1 ? 'pattern' : 'patterns'} in this skill, and Hermes' install policy blocks a
          {scan.trustLevel} skill with a {scan.verdict} verdict. Nothing was written to {destinationLabel}.
        </p>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <div class="space-y-2">
          {#each worstFirst(scan.findings) as f, i (i)}
            <div class="rounded-lg border border-line-subtle p-3">
              <div class="flex items-center gap-2">
                <Chip tone={SEVERITY_TONE[f.severity] ?? 'neutral'}>{f.severity}</Chip>
                <span class="font-mono text-[11px] text-muted">{f.category}</span>
                <span class="min-w-0 flex-1 truncate text-right font-mono text-[11px] text-ink-dim">
                  {f.file}{f.line ? `:${f.line}` : ''}
                </span>
              </div>
              <div class="mt-1.5 font-sans text-sm text-fg">{f.description}</div>
              {#if f.match}
                <div class="mt-1 truncate font-mono text-[11px] text-muted" title={f.match}>{f.match}</div>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {:else}
      <div class="flex items-center gap-3">
        <Input autofocus bind:value={q} placeholder="Search Hermes' skills index: 1password, pdf, kubernetes…" class="max-w-md" />
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
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Hermes skills index</span>
        <InfoTip text="Hermes' official skills index: its own optional skills, the GitHub repos it curates, and skills.sh. Every install is fetched from GitHub and scanned first by Hermes' Skills Guard, with Hermes' rules — a community skill with any high-risk finding is not installed, a trusted one only when nothing critical turns up. Support files ride along and the agent reads them live. Install only what you trust." />
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
      <div class="min-h-0 flex-1 overflow-y-auto">
        {#if resultsQuery.isError}
          <EmptyState
            icon="⚠"
            title="Marketplace didn't load"
            hint={errorMessage(resultsQuery.error) || 'The skills catalog could not be read. Close and reopen the marketplace to retry.'}
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
          <EmptyState icon="✦" title="No skills match" hint="The index lists Hermes' installable skills by name, tag and description. Try a broader search." />
        {:else}
          <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {#each results.entries as e (e.id)}
              {@const done = isDone(e)}
              {@const busy = busyId === e.id}
              {@const outcome = outcomes.get(e.id)}
              <div class="group relative flex flex-col rounded-lg border border-line-subtle p-4 transition-colors hover:border-line dither-fill">
                <div class="flex items-center gap-3">
                  <span class="grid h-[34px] w-[34px] shrink-0 place-items-center rounded-lg border border-line-subtle bg-surface text-muted">
                    <Store size={16} />
                  </span>
                  <div class="min-w-0 flex-1 pr-8">
                    <div class="truncate font-sans text-sm font-semibold text-fg">{e.name}</div>
                    <div class="flex items-center gap-1.5">
                      <span class="truncate font-mono text-[11px] text-muted">{e.source === 'official' ? 'Hermes official' : e.repo}</span>
                      <Chip tone={TRUST_TONE[e.trustLevel] ?? 'neutral'} title={TRUST_NOTE[e.trustLevel]}>{e.trustLevel}</Chip>
                    </div>
                  </div>
                </div>
                <p class="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
                  {e.description || 'No description.'}
                </p>
                {#if outcome === 'exists'}
                  <span class="mt-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted" title="A skill with this name is already there — it was left exactly as it was">
                    already present
                  </span>
                {/if}
                <a
                  href={githubUrl(e)}
                  target="_blank"
                  rel="noreferrer"
                  class="mt-1 inline-flex items-center gap-1 self-start font-mono text-[10px] text-muted opacity-0 transition-opacity hover:text-accent group-hover:opacity-100"
                >
                  <ExternalLink size={10} />
                  {e.repo}{e.path ? `/${e.path}` : ''}
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
                      title={busy ? 'Fetching and scanning the skill' : `Scan and install into ${destinationLabel}`}
                      disabled={busy || busyId !== null}
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
