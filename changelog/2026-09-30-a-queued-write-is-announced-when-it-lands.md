- **A queued Google write now actually tells the person who has to approve it.**
  When an agent drafts an email, edits someone's doc, moves a Drive file or
  changes a calendar event, the approval is announced the moment it lands — the
  agent is stopped in front of it, and the decision usually takes seconds.

  That was supposed to be true already. The code path existed, with a comment
  explaining the five minutes it saves over waiting for the approval sweep, and
  the edge it calls through was declared. Nothing ever set that edge. The call
  site asks for it and does nothing when it is absent, so there was no compile
  error, no warning and no log line — every queued write silently waited for
  the sweep's next tick, exactly the behaviour the comment said had been fixed.

  The boot-wiring test that guards every other edge of this kind now covers this
  one too. That test is the reason the others work, and its gap is the whole
  explanation for this one.

  Verified: every touched api crate parses and is formatted under `cargo fmt
  --check`, and the dependency graph resolves with no cycle; `bun run check`
  green. The assertion runs in CI's api job, where it would have failed before
  this change.
