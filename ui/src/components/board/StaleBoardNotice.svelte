<script lang="ts">
  import QueryError from '@/components/ui/QueryError.svelte'

  // The one banner the board lenses show when a REFERENCE read — the status
  // set behind the columns, the labels behind the pills — failed while the
  // tickets themselves loaded. Three lenses (list, kanban, gantt) each wrote
  // this out; they agreed on the shape and disagreed, twice, on the sentence.
  //
  // The rule the three share, and the reason this is one component: a stale
  // reference read beats no board at all, so the banner never REPLACES the
  // chart — it marks what is old (a lost status set leaves colours and the
  // picker guessing; a lost label set only costs tint). The no-data case is
  // handled by the caller long before here: reaching this component with
  // `statuses.data === undefined` means the caller deliberately kept the
  // board up.

  /** The minimum of a query this banner reads: enough to say whether it
   *  failed and to offer its retry. Deliberately not `QueryLike`: this never
   *  touches data, and taking the wider type would invite it to. */
  interface StaleRead {
    isError: boolean
    data: unknown
    error: unknown
    refetch: () => unknown
  }

  let {
    statuses,
    labels,
    /** What this lens calls the status set on screen ("Columns" in kanban). */
    word = 'Statuses',
    /** A lens with its own sentence for "the status set never arrived". */
    missingTitle,
  }: {
    statuses: StaleRead
    labels?: StaleRead
    word?: string
    missingTitle?: string
  } = $props()

  const title = $derived(
    statuses.isError
      ? statuses.data === undefined && missingTitle
        ? missingTitle
        : `${word} may be out of date`
      : labels?.data === undefined
        ? 'Could not load labels, so the pills below show names without their colours'
        : 'Labels may be out of date',
  )
</script>

{#if statuses.isError || labels?.isError}
  <QueryError
    variant="inline"
    class="border-b border-line-subtle px-4 py-2"
    {title}
    error={statuses.isError ? statuses.error : labels?.error}
    onRetry={() => {
      if (statuses.isError) void statuses.refetch()
      if (labels?.isError) void labels.refetch()
    }}
  />
{/if}
