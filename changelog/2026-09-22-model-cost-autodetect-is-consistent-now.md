- **Model cost autodetect is consistent now.** The price oracle only ever
  priced an endpoint's REGISTERED models — but tier routing and aliases
  attribute usage to models nobody registered (grok via a tier mention,
  gemini "-latest" aliases), which stayed unpriced forever, and id shapes
  like "~"-prefixed aliases, ":free" variants, "-latest", and trailing
  release dates never matched OpenRouter's catalog ids. The oracle now
  prices the union of registered models and every model usage has
  actually landed on (keyed by the exact usage string so the costing
  join hits), matches through alias fallbacks (strip "~", ":variant",
  "-YYYYMMDD", "-latest", vendor-prefixed suffix match), and an unpriced
  cloud usage row nudges a refresh ahead of the 6h cadence (15min
  throttle). Live result: unpriced cloud tokens 13.5M → 0.

### Added
