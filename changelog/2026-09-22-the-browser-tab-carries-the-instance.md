- **The browser tab carries the instance's company name.** Admins can name
  the workspace (Admin → org tab, beside the hosting domain); until then
  the tab reads "Talaria", and once named it reads "Talaria - <name>" — on
  every route and the sign-in page too, because the name rides the public
  identity beacon alongside the instance id rather than an authed read.
  Purely cosmetic: nothing else derives from it. The name trims on save,
  whitespace-only is refused (clearing is an explicit null an accidental
  save can never perform), and every set lands in the audit log.
