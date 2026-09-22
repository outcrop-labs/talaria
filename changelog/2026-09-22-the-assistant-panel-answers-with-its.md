- **The assistant panel answers with its tools now, steered by the view.** The
  sidebar conversation used to open every detached turn with "Tools are
  disabled" — disarming the owner's personal assistant made every live-state
  question ("how many tickets are on Finance?") unanswerable except by
  invention. The reply harness now arms the persona's own governed tool loop,
  and the prompt tells it which tools to reach for FIRST: the ones that match
  the view the panel is floating over (Boards → list_boards/list_tickets/
  get_ticket/comment; Knowledge → search_knowledge and the KB reads; Comms →
  channels and messaging; …), then its other tools when those cannot answer.
  The surface tool lists are server-side, keyed by the same id the briefs use,
  so a client cannot write tool names into the prompt. Inbox queue-card
  sign-offs keep their own path: propose through the command branch, confirm
  with a click — never the detached conversation. Three inbox-reply fixtures
  were reworded to the new contract (unbacked action claims still fail;
  Inbox-card action ids still fail; live-state answers must be grounded in
  tools or hedged, never invented).
