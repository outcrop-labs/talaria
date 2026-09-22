- **`check-docs` no longer reads code spans as links.** A generated schema
  cell like `` `uuid[](50)` `` is markdown's `[text](target)` shape wearing
  backticks; the link checker now masks inline code spans before matching,
  so generated tables don't trip the dead-link rule.

### Removed
