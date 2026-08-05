<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { parseAgentStream } from '@/lib/sse-parse'
  import AssistantCard from './AssistantCard.svelte'
  import { BRIEF_ASK, type BriefScope, type BriefingData } from './home'

  // The assistant briefing: what needs you, in your assistant's voice.
  // Regenerates only when the attention fingerprint changes; chatting back
  // continues the same conversation through the normal chat pipeline.
  let { scope = 'inbox' }: { scope?: BriefScope } = $props()

  const query = createQuery(() => ({
    queryKey: ['briefing', scope],
    queryFn: (): Promise<BriefingData> => getJson<BriefingData>(`/api/me/briefing?scope=${scope}`),
    refetchInterval: (q: { state: { data?: BriefingData } }) => (q.state.data?.generating ? 2_500 : 60_000),
  }))

  let thread = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  let draft = $state('')
  let replying = $state(false)

  const send = async () => {
    const content = draft.trim()
    if (!content || replying || !query.data?.agentModel) return
    const history = [...thread]
    draft = ''
    thread = [...thread, { role: 'user', content }, { role: 'assistant', content: '' }]
    replying = true
    try {
      // Ephemeral by design: streams through the assistant with the briefing
      // as context; nothing is persisted, the thread lives in this panel only.
      const res = await fetch('/api/me/briefing/chat', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ scope, content, history }),
      })
      if (!res.ok || !res.body) throw new Error(`chat failed: ${res.status}`)
      for await (const ev of parseAgentStream(res.body)) {
        if (ev.type === 'content') {
          const next = [...thread]
          const last = next[next.length - 1]!
          next[next.length - 1] = { ...last, content: last.content + ev.text }
          thread = next
        }
      }
    } catch {
      thread = [...thread.slice(0, -1), { role: 'assistant', content: '(could not reply — try again)' }]
    } finally {
      replying = false
    }
  }
</script>

{#if query.isLoading}
  <Panel>
    <div class="mb-3 flex items-center gap-2">
      <Skeleton class="h-5 w-5 rounded-full" />
      <Skeleton class="h-3 w-32 rounded-full" />
    </div>
    <SkeletonRows rows={3} />
  </Panel>
{:else if !query.data}
  <!-- nothing -->
{:else if query.data.none}
  <AssistantCard />
{:else}
  {@const data = query.data}
  <Panel>
    <div class="mb-3 flex items-center gap-2">
      <Sparkles size={15} class="text-accent" />
      <span class="font-sans text-sm font-semibold text-fg">{data.agentName}</span>
      {#if data.generating}
        <span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          <StatusDot status="accent" pulse /> updating
        </span>
      {:else if data.generatedAt}
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(data.generatedAt)}</span>
      {/if}
    </div>
    {#if data.summary}
      <div class="text-sm">
        <Markdown children={data.summary} />
      </div>
    {:else}
      <SkeletonRows rows={3} />
    {/if}
    {#if thread.length > 0}
      <div class="mt-4 space-y-3 border-t border-line pt-4">
        {#each thread as m, i (i)}
          {#if m.role === 'user'}
            <div class="flex justify-end">
              <div class="max-w-[85%] rounded-lg border px-3.5 py-2 font-sans text-sm text-[color:var(--chat-user-foreground)]" style:background="var(--chat-user-bg)" style:border-color="var(--chat-user-border)">
                {m.content}
              </div>
            </div>
          {:else}
            <div class="text-sm">
              {#if m.content}<Markdown children={m.content} />{:else}<SkeletonRows rows={1} />{/if}
            </div>
          {/if}
        {/each}
      </div>
    {/if}
    <div class="mt-4 flex items-end gap-2">
      <Textarea
        autoGrow
        rows={1}
        bind:value={draft}
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault()
            void send()
          }
        }}
        placeholder={`Ask ${data.agentName} ${BRIEF_ASK[scope]}`}
        class="max-h-32 text-sm"
      />
    </div>
  </Panel>
{/if}
