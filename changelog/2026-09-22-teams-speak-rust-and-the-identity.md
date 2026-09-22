- **Teams speak Rust, and the identity-proxy model crosses with them**:
  `/api/teams`, `/api/teams/{id}`, and `/api/teams/{id}/members` — the list
  (with live member counts), create/rename/delete (owner-gated; boards
  survive team deletion as personal boards), and the member plane (add by
  email with re-role on conflict, remove that never removes an owner). The
  list is the port's first ACTING-user surface: a personal assistant calling
  with its `tak_` key acts as its owner, a general agent or a rejected
  credential resolves to nobody, and the owner's admin role gates the
  assistant's elevation. Byte-diffed against TS for anon, member, and admin
  callers plus both agent shapes — reads, write cycles, the 405s with their
  `allow` headers, and the full 400 table (the plain-Email check, the enum
  role with its `.default('member')`, and the uuid body member).
