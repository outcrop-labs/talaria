<script lang="ts">
  import { QueryCache, QueryClient, QueryClientProvider } from '@tanstack/svelte-query'
  import { Router } from 'sv-router'
  import './router'
  import ConfirmHost from '@/components/ui/ConfirmHost.svelte'
  import ErrorFallback from '@/components/ui/ErrorFallback.svelte'
  import TabTitle from '@/components/app/TabTitle.svelte'

  // A query that errors used to fail completely silently — the incident
  // where a 500 from /api/boards rendered as "no boards" left no trace
  // anywhere. This does not change behaviour, it just makes a failed
  // fetch visible in the console with the key that failed.
  const queryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        console.error('[query]', JSON.stringify(query.queryKey), error)
      },
    }),
    defaultOptions: { queries: { staleTime: 5_000, retry: 1 } },
  })

  // The branded tab title is a CHILD of the provider (TabTitle), not a call
  // here in App's script: this script runs above the QueryClientProvider
  // below, where a query has no client to find. Boot order is load-bearing.
</script>

<QueryClientProvider client={queryClient}>
  <TabTitle />
  <!-- Outermost net. Route-level errors land in the router's onError hook;
       this boundary catches everything above it (providers, the shell itself)
       that would otherwise render an empty <body>. -->
  <svelte:boundary onerror={(error) => console.error('[talaria]', error)}>
    <Router />
    <ConfirmHost />
    {#snippet failed(error, reset)}
      <ErrorFallback {error} {reset} what="Talaria" />
    {/snippet}
  </svelte:boundary>
</QueryClientProvider>
