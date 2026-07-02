import { Combobox } from '@/components/ui/combobox'
import type { ControlSize } from '@/components/ui/control'
import type { LlmEndpoint, ModelTarget } from '@/lib/fleet-defs'

// Unit separator — never appears in provider/model ids.
const SEP = '␟'

/** One searchable picker over every provider's model catalog (Models tab
 *  curates it). Picking sets endpoint + model together; the current value
 *  stays selectable even if it has left the catalog. */
export function ModelPicker({
  endpoints,
  value,
  onChange,
  size,
  className,
}: {
  endpoints: LlmEndpoint[]
  value: ModelTarget
  onChange: (t: ModelTarget) => void
  size?: ControlSize
  className?: string
}) {
  const options = endpoints.flatMap((ep) =>
    (ep.models ?? []).map((m) => ({
      value: `${ep.name}${SEP}${m}`,
      label: m,
      sub: `${ep.name} · ${ep.class}`,
    })),
  )
  const cur = value.model ? `${value.endpoint}${SEP}${value.model}` : ''
  if (cur && !options.some((o) => o.value === cur)) {
    options.unshift({ value: cur, label: value.model, sub: `${value.endpoint} · not in catalog` })
  }
  return (
    <Combobox
      options={options}
      selected={cur ? [cur] : []}
      onChange={([v]) => {
        if (!v) return
        const [endpoint, model] = v.split(SEP)
        if (endpoint && model) onChange({ ...value, endpoint, model })
      }}
      size={size}
      placeholder="Pick a model…"
      className={className}
    />
  )
}
