<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ArrowLeft, Check, ExternalLink, ShieldAlert } from '@lucide/svelte'
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
  import { skillTitle } from '@/components/skills/skill-title'
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
   *  official skills index searched from the local mirror. A click fetches
   *  the skill and runs Hermes' own Skills Guard over it. A clean scan
   *  installs straight away. A scan with findings opens the report first,
   *  as Hermes' own install prints it and asks before writing: a skill
   *  Hermes' policy allows installs only on a second, explicit click, and
   *  one it blocks cannot be installed at all. The destination is an owner —
   *  one agent, or the shared root every agent carries. */
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
  // A skill whose scan report is open: one Hermes' policy blocked, or one
  // it allows whose scan still found something. The entry and its preview.
  let report = $state<{ entry: MarketplaceEntry; skill: MarketplaceSkill } | null>(null)
  const reportBlocked = $derived(report !== null && report.skill.scan.allowed !== true)
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
  const TRUST_LABEL: Record<string, string> = { builtin: 'Official', trusted: 'Trusted', community: 'Community' }
  const TRUST_NOTE: Record<string, string> = {
    builtin: 'Maintained by Nous Research with Hermes itself',
    trusted: 'From a repo Hermes trusts (OpenAI, Anthropic, Hugging Face, NVIDIA): installs unless the scan finds something critical',
    community: 'Installs only when the scan finds nothing high-risk',
  }
  const sentence = (word: string) => word.charAt(0).toUpperCase() + word.slice(1)
  const VERDICT_TONE: Record<string, ChipTone> = { safe: 'success', caution: 'warn', dangerous: 'danger' }
  const SEVERITY_TONE: Record<string, ChipTone> = { critical: 'danger', high: 'warn', medium: 'neutral', low: 'neutral' }
  // Worst first, as Hermes' own scan report orders them.
  const SEVERITY_RANK: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 }
  const worstFirst = (findings: ScanFinding[]) =>
    [...findings].sort((a, b) => (SEVERITY_RANK[a.severity] ?? 4) - (SEVERITY_RANK[b.severity] ?? 4))
  const githubUrl = (e: MarketplaceEntry) => `https://github.com/${e.repo}${e.path ? `/tree/HEAD/${e.path}` : ''}`

  const write = async (entry: MarketplaceEntry) => {
    const outcome = await installMarketplaceSkill(destination, entry.id)
    outcomes = new Map(outcomes).set(entry.id, outcome.status)
    await qc.invalidateQueries({ queryKey: SKILLS_KEY })
  }

  /** The card's click: fetch and scan. A clean scan installs; any finding
   *  opens the report, whether the policy allows the skill or blocks it. */
  const install = async (entry: MarketplaceEntry) => {
    busyId = entry.id
    error = null
    try {
      const skill = await marketplaceDetail(destination, entry.id)
      if (skill.scan.allowed !== true || skill.scan.findings.length > 0) {
        report = { entry, skill }
        return
      }
      await write(entry)
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busyId = null
    }
  }

  /** The report's own button, for a skill the policy allows: the member has
   *  read the findings and installs anyway. The api re-scans (from its
   *  cache) and still refuses anything the policy blocks. */
  const installReviewed = async () => {
    if (!report || reportBlocked) return
    const { entry } = report
    busyId = entry.id
    error = null
    try {
      await write(entry)
      report = null
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busyId = null
    }
  }
</script>

<Modal open {onClose} title="Skills marketplace" takeover>
  <div class="flex h-full min-h-0 flex-col gap-5">
    {#if report}
      {@const scan = report.skill.scan}
      {@const count = `${scan.findings.length} ${scan.findings.length === 1 ? 'pattern' : 'patterns'}`}
      <!-- The scan report: a skill Hermes' install policy blocked, or one it
           allows whose scan still found something. -->
      <div class="flex items-center gap-4">
        <Button size="sm" variant="outline" onclick={() => ((report = null), (error = null))}>
          <ArrowLeft size={14} />
          All skills
        </Button>
        <div class="min-w-0 flex-1">
          <div class="truncate font-sans text-sm font-medium leading-5 text-fg">{skillTitle(report.entry.name)}</div>
          <div class="truncate font-mono text-xs leading-4 text-muted">{report.skill.upstream ?? report.entry.repo}</div>
        </div>
      </div>
      <div class={cn('rounded-lg border bg-surface p-5', reportBlocked ? 'border-danger/40' : 'border-warning/40')}>
        <div class="flex items-center gap-2">
          <ShieldAlert size={16} class={reportBlocked ? 'text-danger' : 'text-warning'} />
          <span class="font-sans text-sm font-medium leading-5 text-fg">{reportBlocked ? 'Not installed' : 'Read before installing'}</span>
          <Chip tone={VERDICT_TONE[scan.verdict] ?? 'neutral'}>{sentence(scan.verdict)}</Chip>
          <Chip tone={TRUST_TONE[scan.trustLevel] ?? 'neutral'} title={TRUST_NOTE[scan.trustLevel]}>
            {TRUST_LABEL[scan.trustLevel] ?? scan.trustLevel}
          </Chip>
        </div>
        {#if reportBlocked}
          <p class="mt-2 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
            Hermes' Skills Guard ({scan.scanner}) found {count}. Its policy blocks a {scan.trustLevel} skill with a
            {scan.verdict} verdict, so nothing was written to {destinationLabel}.
          </p>
        {:else}
          <p class="mt-2 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">
            Hermes' Skills Guard ({scan.scanner}) found {count}. Its policy allows a {scan.trustLevel} skill with a
            {scan.verdict} verdict, so you can install it once you have read the report. Nothing is written to
            {destinationLabel} until you do.
          </p>
          <div class="mt-4 flex items-center gap-2">
            <Button size="sm" variant="outline" onclick={() => void installReviewed()} disabled={busyId !== null}>
              {#if busyId === report.entry.id}<WaitingMark site="skills/marketplace-install" size={12} />{/if}
              Install into {destinationLabel}
            </Button>
            <Button size="sm" variant="ghost" onclick={() => ((report = null), (error = null))} disabled={busyId !== null}>
              Cancel
            </Button>
          </div>
        {/if}
        {#if error}<div transition:slide={{ duration: 150 }} class="mt-2 font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
      </div>
      <div class="min-h-0 overflow-y-auto rounded-lg border border-line bg-surface [&>*+*]:border-t [&>*+*]:border-line-subtle">
        {#each worstFirst(scan.findings) as f, i (i)}
          <div class="px-5 py-4">
            <div class="flex items-center gap-2">
              <Chip tone={SEVERITY_TONE[f.severity] ?? 'neutral'}>{sentence(f.severity)}</Chip>
              <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-fg">{f.description}</span>
            </div>
            <div class="mt-1 truncate font-mono text-xs leading-5 text-muted">
              {f.category} · {f.file}{f.line ? `:${f.line}` : ''}
            </div>
            {#if f.match}
              <div class="mt-2 truncate rounded-md border border-line-subtle bg-panel px-2.5 py-1.5 font-mono text-xs text-muted" title={f.match}>
                {f.match}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <div class="flex flex-wrap items-center gap-3">
        <Input autofocus size="sm" bind:value={q} placeholder="Search skills, for example pdf or kubernetes" class="max-w-md" />
        {#if resultsQuery.isFetching}<WaitingMark site="skills/marketplace-search" size={12} class="text-muted" />{/if}
        <span class="flex-1"></span>
        <label class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
          Install into
          <Select size="sm" bind:value={destination} class="w-44">
            {#each destinations as o (o.owner)}
              <option value={o.owner}>{o.owner === 'shared' ? 'Every agent' : o.label}</option>
            {/each}
          </Select>
        </label>
      </div>
      <div class="flex items-center gap-2">
        <span class="font-sans text-[13px] leading-5 text-muted">From the Hermes skills index. Every skill is scanned before it installs.</span>
        <InfoTip text="Hermes' official skills index: its own optional skills, the GitHub repos it curates, and skills.sh. Every install is fetched from GitHub and scanned first by Hermes' Skills Guard, with Hermes' rules: a community skill with any high-risk finding is not installed, a trusted one only when nothing critical turns up. A clean scan installs on the click. Anything the scan found is shown first, and an allowed skill then installs only when you confirm. Support files ride along and the agent reads them live. Install only what you trust." />
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
      <div class="min-h-0 flex-1 overflow-y-auto">
        {#if resultsQuery.isError}
          <EmptyState
            icon="⚠"
            title="Marketplace didn't load"
            hint={errorMessage(resultsQuery.error) || 'The skills catalog could not be read. Close and reopen the marketplace to retry.'}
          />
        {:else if !results}
          <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {#each Array.from({ length: 9 }, (_, i) => i) as i (i)}
              <div class="rounded-lg border border-line bg-surface p-5">
                <Skeleton class="h-3.5 w-32 rounded-full" />
                <Skeleton class="mt-3 h-2.5 w-full rounded-full" />
                <Skeleton class="mt-1.5 h-2.5 w-4/5 rounded-full" />
                <Skeleton class="mt-5 h-2.5 w-24 rounded-full" />
              </div>
            {/each}
          </div>
        {:else if results.entries.length === 0}
          <EmptyState icon="✦" title="No skills match" hint="The index lists skills by name, tag and description. Try a broader search." />
        {:else}
          <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {#each results.entries as e (e.id)}
              {@const done = isDone(e)}
              {@const busy = busyId === e.id}
              {@const title = skillTitle(e.name)}
              <div class="flex flex-col rounded-lg border border-line bg-surface p-5">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg">{title}</span>
                  <Chip tone={TRUST_TONE[e.trustLevel] ?? 'neutral'} title={TRUST_NOTE[e.trustLevel]}>
                    {TRUST_LABEL[e.trustLevel] ?? e.trustLevel}
                  </Chip>
                </div>
                <p class="mt-1 line-clamp-2 min-h-10 font-sans text-[13px] leading-5 text-muted">
                  {e.description || 'No description.'}
                </p>
                <div class="mt-4 flex min-h-8 items-center gap-3">
                  <a
                    href={githubUrl(e)}
                    target="_blank"
                    rel="noreferrer"
                    title={`${e.repo}${e.path ? `/${e.path}` : ''} on GitHub`}
                    class="inline-flex min-w-0 flex-1 items-center gap-1.5 text-muted transition-colors hover:text-fg"
                  >
                    <ExternalLink size={14} class="shrink-0" />
                    {#if e.source === 'official'}
                      <span class="truncate font-sans text-[13px] leading-5">Hermes official</span>
                    {:else}
                      <span class="truncate font-mono text-xs leading-5">{e.repo}</span>
                    {/if}
                  </a>
                  {#if done && !busy}
                    <span
                      class="flex shrink-0 items-center gap-1.5 font-sans text-[13px] leading-5 text-muted"
                      title={outcomes.get(e.id) === 'exists'
                        ? 'A skill with this name was already there. It was left as it was.'
                        : `Already in ${destinationLabel}`}
                    >
                      <Check size={14} class="text-success" />
                      {outcomes.get(e.id) === 'exists' ? 'Already there' : 'Installed'}
                    </span>
                  {:else}
                    <Button
                      size="sm"
                      variant="outline"
                      title={busy ? 'Fetching and scanning the skill' : `Scan ${title} and install it into ${destinationLabel}`}
                      disabled={busy || busyId !== null}
                      onclick={() => void install(e)}
                    >
                      {#if busy}<WaitingMark site="skills/marketplace-install" size={12} />Scanning{:else}Install{/if}
                    </Button>
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
