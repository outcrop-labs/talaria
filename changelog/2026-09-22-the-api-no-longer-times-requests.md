- **The api no longer times requests out.** The Rust router carried a 30s
  `TimeoutLayer` (503 on expiry) that the TS server it replaced never had —
  a port-scaffolding pick that broke every legitimately slow surface it
  touched: long agent turns and tool-call chains, Google syncs, heavy reads.
  Routes it broke had been getting carved out of it one at a time into a
  second "streaming" stack until the split was mostly scar tissue. The layer
  is gone and the two stacks are merged back into one: requests fail on
  errors — panics, refused guards, upstream call budgets — never on a clock.
