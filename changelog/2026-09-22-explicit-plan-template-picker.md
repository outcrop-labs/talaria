- **Explicit plan-template picker.** Plans could only seed their living document
  from the agent's bound plan template, implicitly — the missing half of the
  chain tickets already had. The Plan surface now shows a Template picker (in
  the header, for a new plan before its first turn) listing every plan-kind
  template; the pick rides through plan creation onto the conversation
  (`conversations.plan_template_id`) and becomes the highest link in
  `resolveTemplate` — explicit pick → agent binding → none. "Automatic" leaves
  it to the agent default. The chosen skeleton seeds the doc verbatim and
  shapes every later agent rewrite. Verified live: a plan created with an
  explicit template seeds its doc from that skeleton, automatic falls through
  to the agent binding, and the pick persists on the conversation, 7/7 checks.
