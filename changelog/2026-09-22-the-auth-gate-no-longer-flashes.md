- **The auth gate no longer flashes the dashboard before login.** The app shell
  painted its skeleton (rail, strip, content cards) while the session read was
  in flight and in the beat before the /login navigation landed — fine for a
  signed-in reload, the dashboard flashing at every signed-out visitor. The
  gate now holds on the Mercury ground with the brand mark centered until the
  session resolves; a failed read keeps the real chrome with its retry, as
  before.
