# Apps

Apps are self-contained additions that compile into this deployment and render as native
Talaria surfaces — built by your team, the community, or Outcrop. They run under each
signed-in user's session, so platform permissions apply unchanged. An enabled app appears in
the sidebar with its own name (between Work and Manage), but only for people it's been
granted to: apps are explicit-grant, and a member gets nothing by default.

Open: **Manage → Apps**. Installing, enabling, and uninstalling are admin-only; see
[The lifecycle](#the-lifecycle).

## To…

| Do this | How |
| :--- | :--- |
| See what's installed | **Installed** tab — icon, name, `v{version}`, description, and surface chips |
| Open an app's surface | Its **Open** link on the card, or its name in the sidebar |
| Discover more | **Discover** tab — community and official apps from the marketplace index |
| Install one (admin) | **Install** on a Discover card, or **Install from Git** with any repository that has a `talaria.json` at its root |
| Activate a fresh install | Reload the dev server or rebuild the deployment — then **Enable** it under Installed |
| Turn one off (admin) | **Disable** on the card — no nav presence, its API routes 404, its MCP server retires |
| Remove one (admin) | Trash icon → **Uninstall**. Removes the codebase and deletes the data it stored; this cannot be undone |
| Get access to an app | Ask your admin — each app view is granted per person in **Admin → People**, same flow as core Manage views |

## Reading a card

| Chip | Means |
| :--- | :--- |
| **work** | A Work view: the surface the app is for |
| **manage** | A Manage view: the app's own admin surface |
| **settings** | A panel that appears in Settings |
| **mcp** | Publishes MCP tools for agents — govern access in Manage → MCP |
| **awaiting build** | Installed on disk but not compiled into this build yet; reload the dev server or rebuild to activate |
| **official** | Maintained by Outcrop Labs |

## The lifecycle

| Stage | What happens |
| :--- | :--- |
| **Install** | Clones the app's repository into this deployment |
| **Build** | The app compiles into the deployment — pending ones show **awaiting build** |
| **Enable** | The app's surfaces and MCP server come alive |
| **Grant** | Admins allow each view per person — enabling alone gives members nothing |
| **Uninstall** | Removes the codebase and deletes stored data. Audited |

One honest warning, in the product's own words: installing an app means its code runs fully
trusted, like the platform itself — **install only apps you trust**.

## The apps that ship with Talaria

| App | What it is |
| :--- | :--- |
| **Contacts** `☏` | A lightweight CRM — people, companies, stages, notes. The reference Talaria app |
| **LeadWorks** `◍` | A demand-engine POC — AI search visibility, inbound qualification, outbound generation, one sales handoff |

## Where an app's data lives

Each app gets its own document store — its data does not land in your Files or Knowledge.
Agents reach that data through the app's MCP tools (a Contacts agent can search and add
contacts), governed in **Manage → MCP** exactly like any server.

## Words Apps uses

| Term | Meaning |
| :--- | :--- |
| **Surface** | One render target of an app: work, manage, or settings |
| **Explicit grant** | The default-deny rule: every app view must be allowed per person |
| **Marketplace** | The catalog the Discover tab reads; any git repository with a `talaria.json` at its root can be an app |
