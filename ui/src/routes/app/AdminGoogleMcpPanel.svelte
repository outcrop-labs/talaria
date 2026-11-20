<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import CodeBlock from '@/components/ui/CodeBlock.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import type { ChipTone } from '@/components/ui/chip'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  // GOOGLE WORKSPACE MCP (Developer Preview) — Google's own MCP servers for
  // Gmail, Drive, Calendar and the rest, behind Talaria's MCP gateway. Off
  // until an admin enables a product here; an enabled product reaches only
  // the agents it is then assigned to on the MCP page, and each agent acts
  // with its own Google principal (its owner's connection, the org account,
  // or its own identity). Per tool: the pinned reads Google marks read-only
  // run; trash, spam and sensitive labels are denied; every other tool is
  // HELD — each call waits in the approval queue for a person (the owner for
  // a personal assistant, an admin for the org account or an agent's own
  // identity). An admin may override a tool's default here, except that the
  // deny list stays denied, and that "runs" reaches only the org account and
  // agents' own identities: a personal assistant's call into its owner's own
  // Google account still waits for that owner.
  type Policy = 'allow' | 'hold' | 'deny'
  interface Tool {
    name: string
    title: string | null
    readOnlyHint: boolean | null
    destructiveHint: boolean | null
    pinnedRead: boolean
    /** On the owner's deny list: no override lifts it. */
    denyListed: boolean
    /** The pack's default for the tool. */
    policy: Policy
    /** The admin's override, when there is one. */
    override: Policy | null
    /** What the gateway does with it now, for the org account and agents' own identities. */
    effective: Policy
    /** …and for a personal assistant, where an admin's “runs” does not reach. */
    effectivePersonal: Policy
    /** Google's own list of the scopes it accepts for the tool. */
    requiredScopesAnyOf: string[] | null
  }
  interface Product {
    id: string
    name: string
    endpoint: string
    mcpService: string
    restService: string | null
    enabled: boolean
    server: {
      id: string
      name: string
      label: string
      enabled: boolean
      allAgents: boolean
      toolsRefreshedMs: number | null
      probe: { state: string; detail: string; at: string } | null
    } | null
    tools: Tool[]
  }
  interface Payload {
    previewTerms: string
    previewTermsUrl: string
    products: Product[]
    refusals: Array<{
      principal: 'owner' | 'org' | 'agent'
      who: string | null
      status: number | null
      text: string | null
      tool: string | null
      at: string | null
      flagged: boolean
    }>
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['google-mcp'],
    queryFn: (): Promise<Payload> => getJson<Payload>('/api/admin/google/mcp'),
  }))
  let busy = $state<string | null>(null)
  let error = $state<string | null>(null)

  const POLICY_TONE: Record<Policy, ChipTone> = { allow: 'success', hold: 'warn', deny: 'danger' }
  const POLICY_LABEL: Record<Policy, string> = { allow: 'runs', hold: 'held for a person', deny: 'denied' }
  const policyTitle = (t: Tool) =>
    t.override && t.override === t.effective
      ? `An admin set this tool to “${POLICY_LABEL[t.override]}” (the default is “${POLICY_LABEL[t.policy]}”).${
          t.effectivePersonal !== t.effective
            ? ` A personal assistant’s call is still ${POLICY_LABEL[t.effectivePersonal]}: its owner approves each one.`
            : ''
        }`
      : t.effective === 'allow'
        ? 'On the pinned read list and marked read-only by Google: relayed to Google.'
        : t.effective === 'deny'
          ? 'Never run for an agent: trash, spam and sensitive labels.'
          : t.pinnedRead
            ? 'On the read list, but Google’s catalog does not mark it read-only now: each call waits for a person.'
            : 'Changes something, or is new: each call waits in the approval queue until a person approves it.'
  const setPolicy = async (p: Product, t: Tool, select: HTMLSelectElement) => {
    const value = select.value
    const next: Policy | null = value === 'default' ? null : (value as Policy)
    if (next === 'allow' && t.policy !== 'allow') {
      const ok = await confirm({
        title: `Let agents run ${t.name} without a person`,
        message: `${t.name} on ${p.name} ${t.readOnlyHint ? 'is marked read-only by Google but is not on Talaria’s pinned read list' : 'can change something in Google Workspace'}. Set to “runs”, the org account and every agent with its own Google identity assigned this server call it with no approval. A personal assistant’s call into its owner’s own Google account still waits for that owner: an admin never approves those. Google calls its tool annotations untrusted hints.`,
        confirmLabel: 'Let it run',
        danger: true,
      })
      if (!ok) {
        select.value = t.override ?? 'default'
        return
      }
    }
    busy = `${p.id}/${t.name}`
    error = null
    try {
      await putJson<unknown>(`/api/admin/google/mcp/${p.id}/tools/${encodeURIComponent(t.name)}`, { policy: next })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['google-mcp'] })
  }
  const scopeShort = (s: string) => s.replace('https://www.googleapis.com/auth/', '')

  const enabledServices = (d: Payload) => {
    const list = d.products.filter((p) => p.enabled)
    const from = list.length ? list : d.products
    return from.flatMap((p) => [p.mcpService, p.restService].filter((s): s is string => !!s))
  }

  const setEnabled = async (p: Product, next: boolean, d: Payload) => {
    if (next) {
      const ok = await confirm({
        title: `Enable ${p.name} (Developer Preview)`,
        message: `Google's Workspace MCP servers are in Developer Preview. Google's terms say: «${d.previewTerms}» (${d.previewTermsUrl}). Enabling adds ${p.name} to the MCP page, assigned to no agent; agents you assign it to run its read-only tools as their own Google account, and every other call waits for a person to approve it.`,
        confirmLabel: 'Enable',
      })
      if (!ok) return
    } else {
      const ok = await confirm({
        title: `Disable ${p.name}`,
        message: `Remove ${p.name}'s Google MCP server? Every agent it is assigned to drops it, and its assignments and tool overrides go with it; calls still waiting for approval fail when approved.`,
        confirmLabel: 'Disable',
        danger: true,
      })
      if (!ok) return
    }
    busy = p.id
    error = null
    try {
      if (next) await postJson<unknown>(`/api/admin/google/mcp/${p.id}`, { acknowledgePreview: true })
      else await delJson<unknown>(`/api/admin/google/mcp/${p.id}`)
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['google-mcp'] })
  }
</script>

<Panel class="mt-4">
  <QueryState query={query} errorTitle="Could not load Google Workspace MCP" errorVariant="compact">
    {#snippet skeleton()}
      <Skeleton class="mb-3 h-4 w-40 rounded-full" />
      <SkeletonRows rows={3} />
    {/snippet}
    {#snippet children(data)}
      <SectionHeader
        title="Google Workspace MCP · Developer Preview"
        info="Google's own MCP servers, behind Talaria's MCP gateway. Each agent you assign a server to acts with its own Google principal — its owner's connection, the org account, or its own identity — and its refresh token never leaves this server. Read-only tools run; every other call waits for a person to approve it."
      />
      <div class="space-y-4">
        <!-- The preview terms, in Google's words, before anything else. -->
        <div class="flex items-start gap-2 rounded-md border border-warning/40 px-3 py-2 text-xs text-fg">
          <StatusDot status="warn" class="mt-1" />
          <span class="min-w-0">
            Google's Workspace MCP servers are in Developer Preview. Google's terms say:
            <span class="break-words">«{data.previewTerms}»</span>
            <a href={data.previewTermsUrl} target="_blank" rel="noreferrer" class="text-accent underline-offset-2 hover:underline">Terms and enrolment</a>.
            Your Google Cloud project and Workspace must be enrolled, the services below enabled, and the connections client marked
            “Use this client for an AI-powered agent”. Off on this instance until you enable a server.
          </span>
        </div>
        {#if error}
          <div class="flex items-start gap-2 text-xs text-danger"><StatusDot status="danger" class="mt-1" /><span>{error}</span></div>
        {/if}

        <ul class="divide-y divide-line-subtle">
          {#each data.products as p (p.id)}
            <li class="space-y-2 py-2.5">
              <div class="flex flex-wrap items-center gap-2">
                <Toggle
                  checked={p.enabled}
                  onChange={(next) => void setEnabled(p, next, data)}
                  disabled={busy !== null}
                  label={p.name}
                />
                <span class="font-mono text-[10px] text-muted">{p.mcpService}</span>
                {#if p.server?.probe}
                  <span class="ml-auto flex min-w-0 items-center gap-1.5 text-[11px] text-muted">
                    <StatusDot status={p.server.probe.state === 'ok' ? 'ok' : 'warn'} />
                    <span class="truncate">{p.server.probe.detail}</span>
                    <span class="shrink-0 font-sans text-[10px]">· {relativeTime(p.server.probe.at)}</span>
                  </span>
                {/if}
              </div>
              {#if p.enabled}
                <div class="pl-1 font-sans text-[11px] text-muted">
                  Assign <code>{p.server?.name}</code> to agents on the MCP page. A tool marked “runs” reaches Google directly; a
                  held call waits in the approval queue — the owner approves a personal assistant's, an admin the org account's and an
                  agent identity's.
                </div>
                {#if p.tools.length}
                  <Disclosure
                    title={`${p.tools.filter((t) => t.effective === 'allow').length} run · ${p.tools.filter((t) => t.effective === 'hold').length} held · ${p.tools.filter((t) => t.effective === 'deny').length} denied`}
                  >
                    <ul class="space-y-1.5 py-1">
                      {#each p.tools as t (t.name)}
                        <li class="flex flex-wrap items-center gap-2 text-xs">
                          <span class="font-mono text-fg">{t.name}</span>
                          <Chip tone={POLICY_TONE[t.effective]} title={policyTitle(t)}>{POLICY_LABEL[t.effective]}{t.override ? ' · set here' : ''}{t.effectivePersonal !== t.effective ? ' · personal: held' : ''}</Chip>
                          <Select
                            size="sm"
                            class="w-40"
                            aria-label={`Policy for ${t.name}`}
                            value={t.override ?? 'default'}
                            disabled={busy !== null || t.denyListed}
                            title={t.denyListed ? 'On the owner’s deny list: no override lifts it.' : undefined}
                            onchange={(e) => void setPolicy(p, t, e.currentTarget)}
                          >
                            <option value="default">Default ({POLICY_LABEL[t.policy]})</option>
                            <option value="allow">Runs</option>
                            <option value="hold">Held for a person</option>
                            <option value="deny">Denied</option>
                          </Select>
                          {#if t.requiredScopesAnyOf?.length}
                            <span class="min-w-0 truncate font-sans text-[10px] text-muted" title={t.requiredScopesAnyOf.join('\n')}>
                              one of: {t.requiredScopesAnyOf.map(scopeShort).join(', ')}
                            </span>
                          {/if}
                        </li>
                      {/each}
                    </ul>
                  </Disclosure>
                {/if}
              {/if}
            </li>
          {/each}
        </ul>

        <div class="space-y-1.5 border-t border-line pt-3">
          <div class="flex items-center gap-2 text-xs text-fg">
            In Google Cloud, enable each server's MCP service and its product API
            <InfoTip text="Two layers per product: the product's REST API and its separate MCP service. People has no MCP service of its own; Universal Search uses workspacemcp. Chat also needs a Chat app with interactive features turned off." />
          </div>
          <CodeBlock code={`gcloud services enable ${enabledServices(data).join(' ')}`} language="sh" />
        </div>

        {#if data.refusals.length}
          <div class="space-y-1.5 border-t border-line pt-3">
            <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Google's last refusal, per connection</div>
            {#each data.refusals as r, i (i)}
              <div class="flex items-start gap-2 text-xs">
                <StatusDot status="warn" class="mt-1" />
                <span class="min-w-0 text-fg">
                  <span class="font-mono text-[10px] uppercase text-muted">{r.principal}</span>
                  {r.who ?? '—'}{#if r.tool} · <code>{r.tool}</code>{/if}: HTTP {r.status}
                  {#if r.text}<span class="break-words">«{r.text}»</span>{/if}
                  {#if r.flagged}<span class="font-sans text-[10px] text-warning">· a fresh token was refused too: needs a reconnect</span>{/if}
                  {#if r.at}<span class="font-sans text-[10px] text-muted">· {relativeTime(r.at)}</span>{/if}
                </span>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/snippet}
  </QueryState>
</Panel>
