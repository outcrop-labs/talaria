- **Personal agents get a private RAG brain.** Every user's personal
  collection ("My knowledge") is now the assistant's long-term memory of
  that user, created lazily and bound to the owner + their assistant only:
  chat distills land there (alongside the owner-scoped ambient copy —
  search merges dedupe), and personal research reports index there instead
  of sitting unreachable in the activity index. A personal assistant now
  retrieves as its **owner's proxy** — the owner's channels, boards, plans,
  and distilled history, exactly what the owner could retrieve and nothing
  more (org agents keep their board-policy scope). Org-wide research
  reports get an `orgWide` payload + matching scope clause, so they're
  finally retrievable at all. Also fixes agent-key RAG search 502ing on a
  uuid cast in the team-binding clause.
