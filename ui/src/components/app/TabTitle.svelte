<script lang="ts">
  // The tab title, branded by the public identity beacon: "Talaria" alone,
  // or "Talaria - <company>" once an admin names the instance (the read is
  // pre-login, so the sign-in tab is branded too).
  //
  // A COMPONENT, not a line in App.svelte's script, because the beacon read
  // is a query and the query client's context only exists BELOW the
  // QueryClientProvider App renders — App's own script runs above it, where
  // there is no client yet. Until the beacon answers — or if it never does —
  // tabTitle's fallback keeps the bare product name.
  import { useInstanceBranding } from '@/lib/instance-branding.svelte'
  import { tabTitle } from '@/lib/tab-title'

  const branding = useInstanceBranding()
  const title = $derived(tabTitle(branding.data?.companyName))
</script>

<svelte:head>
  <title>{title}</title>
</svelte:head>
