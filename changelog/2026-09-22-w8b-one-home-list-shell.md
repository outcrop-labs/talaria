- **W8b — one home list shell.** `components/app/HomeListPanel.svelte` absorbs the
  scaffolding the four home tabs shared; the tabs keep their headings, empty
  states, row markup, queries and menu controllers. The notice is passed in as
  `notice={list.notice}` **at each tab's own call site**, so
  `check-invariants.mjs`'s `listquery-notice-dropped` rule still finds its literal
  (verified by running the rule's own regex over the four files).
