- **The app talks like a teammate now.** Every piece of user-facing copy was
  swept: "command your fleet" language is gone (the login line is "Sign in
  and get to work.", the brand tagline is "get things done together"), the
  word "fleet" retired from anything a person reads in favor of your agents
  and the team, em dashes came out of copy (~430 across components, routes,
  and the server strings the UI shows, replaced with plain punctuation:
  two sentences, a colon, a comma), and the stiffest sentences were
  shortened and loosened. What was deliberately kept: the em dash as a
  "no value" glyph in tables, en dashes in ranges, LLM prompt prose, and
  code comments. Verified: svelte-check 0 errors, the full suite green
  (2569 tests), and a residual scan shows no em dashes or fleet words left
  in rendered copy.
