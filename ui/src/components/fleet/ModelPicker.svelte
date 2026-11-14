<script lang="ts">
  import { createRawSnippet, mount, unmount, type Snippet } from 'svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ComboOption } from '@/components/ui/combobox'
  import type { ControlSize } from '@/components/ui/control'
  import ProviderMark from '@/components/fleet/ProviderMark.svelte'
  import type { LlmEndpoint, ModelTarget } from '@/lib/fleet-defs'
  import { useModelNames } from '@/components/models/model-names.svelte'

  // Unit separator — never appears in provider/model ids.
  const SEP = '␟'

  // ComboOption.icon is a zero-arg snippet; bridge a <ProviderMark> into one
  // (createRawSnippet renders a shell, the component mounts inside it).
  const markIcon = (provider: string, name?: string): Snippet =>
    createRawSnippet(() => ({
      render: () => '<span style="display:contents"></span>',
      setup(target) {
        const mark = mount(ProviderMark, { target, props: { provider, name } })
        return () => unmount(mark)
      },
    }))

  /** One searchable picker over every provider's model catalog (Models tab
   *  curates it). Picking sets endpoint + model together; the current value
   *  stays selectable even if it has left the catalog. */
  let {
    endpoints,
    value,
    onChange,
    size,
    class: className,
  }: {
    endpoints: LlmEndpoint[]
    value: ModelTarget
    onChange: (t: ModelTarget) => void
    size?: ControlSize
    class?: string
  } = $props()

  const cur = $derived(value.model ? `${value.endpoint}${SEP}${value.model}` : '')

  // A model the catalog names leads with that name; its id moves to the second
  // line, where a search still finds it.
  const modelName = useModelNames()
  const hosting = (ep: LlmEndpoint) => (ep.class === 'local' ? 'Self-hosted' : 'Cloud')

  const options = $derived.by(() => {
    const opts: ComboOption[] = endpoints.flatMap((ep) =>
      (ep.models ?? []).map((m) => {
        const name = modelName(m)
        return {
          value: `${ep.name}${SEP}${m}`,
          label: name ?? m,
          sub: [name ? m : null, ep.name, hosting(ep)].filter(Boolean).join(' · '),
          icon: markIcon(ep.provider, ep.name),
        }
      }),
    )
    if (cur && !opts.some((o) => o.value === cur)) {
      const ep = endpoints.find((e) => e.name === value.endpoint)
      opts.unshift({
        value: cur,
        label: modelName(value.model) ?? value.model,
        sub: `${value.endpoint} · Not in the catalog`,
        icon: markIcon(ep?.provider ?? 'custom', value.endpoint),
      })
    }
    return opts
  })
</script>

<Combobox
  {options}
  selected={cur ? [cur] : []}
  onChange={([v]) => {
    if (!v) return
    if (v.includes(SEP)) {
      const [endpoint, model] = v.split(SEP)
      if (endpoint && model) onChange({ ...value, endpoint, model })
    } else {
      // Typed via "Create" — a model id not in any catalog (e.g. a model
      // released five minutes ago). Keep the row's current endpoint.
      const endpoint = value.endpoint || endpoints[0]?.name
      if (endpoint) onChange({ ...value, endpoint, model: v })
    }
  }}
  allowCreate
  {size}
  placeholder="Pick a model"
  class={className}
/>
