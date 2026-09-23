- **W9d — one agent item.** `components/fleet/AgentItem.svelte` owns the shared
  derivation (`manage`, `running`, `am`) and the modal tail; the list row and the
  tile pass their own frame as a snippet. No asymmetry to paper over: both
  derived the same three bindings from the same inputs.
