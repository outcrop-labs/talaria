<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { TAG_TONE, rankTags, tagLabel, tagTitle, visibleTags, type ModelRow } from './fitness'

  // Capability tags for one model, wherever a model is PICKED — the roles
  // panel, the platform agents panel, member access.
  //
  // THREE STATES, NOT TWO. A tag says known-true, known-false, or never
  // measured, because those are three different facts and `missingCapabilities`
  // only ever treats the middle one as a lack. Rendering unknown as a "no"
  // would paint a fresh self-host — where nothing has been probed — as a
  // catalogue of broken models.
  //
  // A model with NOTHING measured collapses to one neutral "Untested" chip
  // rather than nine grey ones: nine chips that all say "we don't know" is
  // noise wearing the costume of information.
  let {
    row,
    /** Show measured-false tags only. For dense rows where the useful signal is
     *  "what will break", not the full record. */
    negativeOnly = false,
    /** Show this many tags, what the model lacks first, and fold the rest into
     *  one "+N" chip whose title names them. */
    max,
    /** A column of a list, where every row has capabilities: what a model
     *  lacks, or the deployment supplies, stays a chip, and what it has is
     *  one quiet line of words (the first `max`, then a count). A model with
     *  nothing measured says so in the same quiet words. */
    quiet = false,
    class: className = '',
  }: { row: ModelRow | undefined; negativeOnly?: boolean; max?: number; quiet?: boolean; class?: string } = $props()

  const tags = $derived(visibleTags(row))
  // `negativeOnly` is "what will break here", so a capability the DEPLOYMENT
  // supplies is not in it — that is the opposite of broken. It is exactly the
  // case a dense row would otherwise print as "No vision" on an install where
  // vision demonstrably works.
  const ranked = $derived(rankTags(negativeOnly ? tags.measured.filter((c) => c.state === 'no') : tags.measured))
  const shown = $derived(max === undefined ? ranked : ranked.slice(0, max))
  const folded = $derived(max === undefined ? [] : ranked.slice(max))

  const exceptions = $derived(ranked.filter((c) => c.state !== 'yes'))
  const has = $derived(ranked.filter((c) => c.state === 'yes'))
  const hasShown = $derived(max === undefined ? has : has.slice(0, max))
  const unmeasured =
    "Nothing has measured this model's capabilities. Unknown is not a no: Talaria will still run it, and the fitness probes are what turn this into facts."
</script>

{#if quiet}
  <span class="inline-flex flex-wrap items-center gap-x-2 gap-y-1 {className}">
    {#if row?.pooled}
      <Chip title="This id is served by {row.endpoints.length} endpoints ({row.endpoints.join(', ')}), and capability is a property of the endpoint. Test the endpoint-qualified ids to record facts.">
        Pooled
      </Chip>
    {/if}
    {#if !row || !tags.anyMeasured}
      <span class="font-sans text-[13px] leading-5 text-muted" title={unmeasured}>Not measured</span>
    {:else}
      {#each exceptions as c (c.cap)}
        <Chip tone={TAG_TONE[c.state]} title={tagTitle(c)}>{tagLabel(c)}</Chip>
      {/each}
      {#if has.length > 0}
        <span class="font-sans text-[13px] leading-5 text-muted" title={has.map(tagLabel).join(', ')}>
          {hasShown.map(tagLabel).join(', ')}{has.length > hasShown.length ? ` and ${has.length - hasShown.length} more` : ''}
        </span>
      {/if}
    {/if}
  </span>
{:else if row && (row.pooled || !tags.anyMeasured || shown.length > 0)}
  <span class="inline-flex flex-wrap items-center gap-1 {className}">
    {#if row.pooled}
      <!-- A bare id served by several endpoints. Capability is a property of
           the endpoint, so its facts can genuinely differ per member and a
           probe run refuses to write under a pooled key at all. -->
      <Chip title="This id is served by {row.endpoints.length} endpoints ({row.endpoints.join(', ')}), and capability is a property of the endpoint. Test the endpoint-qualified ids to record facts.">
        Pooled
      </Chip>
    {/if}
    {#if !tags.anyMeasured}
      <Chip title={unmeasured}>Untested</Chip>
    {:else}
      {#each shown as c (c.cap)}
        <Chip tone={TAG_TONE[c.state]} title={tagTitle(c)}>{tagLabel(c)}</Chip>
      {/each}
      {#if folded.length > 0}
        <Chip title={folded.map(tagLabel).join(', ')}>+{folded.length}</Chip>
      {/if}
    {/if}
  </span>
{/if}
