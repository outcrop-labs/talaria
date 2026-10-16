<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import type { ResearchSource } from '@/lib/research'

  // THE RUN'S SOURCE REGISTRY, read as a list instead of as the appendix the
  // report body already carries (`extend_report` writes a `## Sources`
  // section into the document). Same rows, but numbered, clickable and
  // skimmable beside the prose rather than at the end of it.
  //
  // It is a dumb list on purpose: whoever holds the run query hands it the
  // sources, so this renders equally well under a finished report, in a
  // follow-up's pane, or anywhere else the rows are already in hand.
  let { sources }: { sources: ResearchSource[] } = $props()

  // A url with no title reads as a string, not as a source, so the host
  // stands in for one — "nytimes.com", not "https://www.nytimes.com/2026/…".
  const host = (url: string) => {
    try {
      return new URL(url).hostname.replace(/^www\./, '')
    } catch {
      // Not every stored url parses (an agent can hand back something
      // malformed); showing it raw beats showing nothing.
      return url
    }
  }
  const label = (s: ResearchSource) => s.title?.trim() || host(s.url)
</script>

<!-- Nothing to say when the registry is empty — a sources panel with no
     sources is chrome pretending to be content. -->
{#if sources.length > 0}
  <Panel class="space-y-3">
    <div class="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-dim">Sources</div>
    <ol class="space-y-2.5">
      <!-- THE NUMBER IS THE REPORT'S, not this list's position: the body
           cites [3], so the row explaining [3] prints 3 even when the set
           arrives sparse. That also means a row the report never cited keeps
           its number — the index belongs to the registry, not to the prose. -->
      {#each sources as s (s.idx)}
        <li class="flex gap-2.5">
          <span class="w-5 shrink-0 text-right font-mono text-[11px] leading-5 text-ink-dim">{s.idx}</span>
          <div class="min-w-0 space-y-0.5">
            <a
              href={s.url}
              target="_blank"
              rel="noopener noreferrer"
              class="block truncate font-sans text-[13px] leading-5 text-fg underline-offset-2 hover:underline"
            >
              {label(s)}
            </a>
            <!-- The host repeats only when the title was the thing shown
                 above; when it already IS the label, saying it twice is noise. -->
            {#if s.title?.trim()}
              <div class="truncate font-mono text-[10px] text-ink-dim">{host(s.url)}</div>
            {/if}
            {#if s.snippet}<p class="line-clamp-2 font-sans text-xs text-muted">{s.snippet}</p>{/if}
          </div>
        </li>
      {/each}
    </ol>
  </Panel>
{/if}
