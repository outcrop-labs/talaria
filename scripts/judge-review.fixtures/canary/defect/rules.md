# AGENTS.md — the canary's rules

## The rules that aren't style

- Every query binds its parameters; request input is never formatted or concatenated into SQL.
- A delete checks that the caller owns the row before it deletes it.
