- **Loading-state audit, platform-wide (~70 gaps, 35 files).** Every
  fetch-backed component now holds a layout-matched skeleton while in
  flight; EmptyStates render only after a query RESOLVES empty. Parent
  queries no longer block unrelated siblings (plan stage, models panels,
  boards' serial fetch, comms message pane). Real bugs flushed out along
  the way: clicking an agent before conversations resolved silently
  started a NEW thread instead of resuming the working one; the memory
  quick-add could clobber the file if used mid-load; a fleet-wide cron
  created while the roster loaded would target nobody; false states
  ("Not connected", "API keys are not enabled", unchecked policy
  checkboxes) flashed during every load.

### Changed
