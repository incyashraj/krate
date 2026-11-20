# Krate in the official MCP Registry

**Not published yet.** `server.json` here is the entry for the hub's remote MCP
server (`https://hub.krate.tech/mcp`, streamable HTTP, OAuth sign-in) in the
official MCP Registry, https://registry.modelcontextprotocol.io. Nothing in this
folder has been sent anywhere.

It follows the 2025-12-11 schema
(https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json)
and validates against it. A remote-only server needs no `packages` entry, so
there is no npm or PyPI ownership check; the registry checks only that the
publisher controls the namespace in `name`
(https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/remote-servers.mdx).

Keep `version` in step with `MCP_SERVER_VERSION` in `../src/mcp.js`, and publish
again whenever it changes. `description` is capped at 100 characters by the
schema.

## Which name, which proof

The name is `tech.krate/krate`: the reverse-DNS form of krate.tech. That
namespace needs proof that the publisher controls krate.tech, by DNS or by HTTP
(https://github.com/modelcontextprotocol/registry/blob/main/docs/modelcontextprotocol-io/authentication.mdx).
It is the better name: people see that the name and the server address belong
to the same publisher.

The fallback is `io.github.incyashraj/krate`, proved by signing in to GitHub as
incyashraj with `mcp-publisher login github`. Change `name` to that before
publishing if you take this route.

## Steps to publish (the founder runs these)

1. Install the publisher:

   ```sh
   brew install mcp-publisher
   ```

2. Make a key pair, outside the repository. macOS's own `openssl` cannot make
   Ed25519 keys; use Homebrew's OpenSSL 3:

   ```sh
   mkdir -p ~/.krate-mcp-registry && cd ~/.krate-mcp-registry
   OPENSSL=/opt/homebrew/opt/openssl@3/bin/openssl   # brew install openssl@3
   $OPENSSL genpkey -algorithm Ed25519 -out key.pem
   PUBLIC_KEY="$($OPENSSL pkey -in key.pem -pubout -outform DER | tail -c 32 | base64)"
   echo "v=MCPv1; k=ed25519; p=${PUBLIC_KEY}"
   ```

   `key.pem` is the private key. Never commit it.

3. Prove krate.tech, one of two ways:

   - **DNS:** add a TXT record on the apex `krate.tech` (not on a
     subdomain) with the value the last command printed:
     `v=MCPv1; k=ed25519; p=<PUBLIC_KEY>`. Then:

     ```sh
     MCP_PRIVATE_KEY="$($OPENSSL pkey -in key.pem -noout -text | sed -n '/priv:/,/pub:/p' | sed '1d;$d' | tr -d ' :\n')" \
       mcp-publisher login dns --domain krate.tech
     ```

   - **HTTP:** serve that same line as the whole body of
     `https://krate.tech/.well-known/mcp-registry-auth` (status 200, no
     redirect). krate.tech is GitHub Pages and the deploy already keeps
     `.well-known/` (pages.yml, "Archive the site, keeping .well-known"), so
     this is one file, `docs/landing/.well-known/mcp-registry-auth`, and one
     site deploy. Check it the way the registry fetches it:

     ```sh
     curl -sS -H 'Accept: text/plain' -A 'mcp-registry/1.0' https://krate.tech/.well-known/mcp-registry-auth
     ```

     Then:

     ```sh
     MCP_PRIVATE_KEY="$($OPENSSL pkey -in key.pem -noout -text | sed -n '/priv:/,/pub:/p' | sed '1d;$d' | tr -d ' :\n')" \
       mcp-publisher login http --domain krate.tech
     ```

4. Publish, from this folder:

   ```sh
   cd cloud/worker/mcp-registry
   mcp-publisher publish
   ```

5. Check it is listed:

   ```sh
   curl "https://registry.modelcontextprotocol.io/v0.1/servers?search=tech.krate/krate"
   ```

On 2026-11-21 a search of the registry for "krate" found no Krate entry and
nothing under `tech.krate`.

## Other places people and AI tools find MCP servers

Nothing has been submitted to any of these. Checked 2026-11-21.

| Where | How a server gets in | What it needs |
| --- | --- | --- |
| Official MCP Registry, https://registry.modelcontextprotocol.io | `mcp-publisher publish` (above) | `server.json` and namespace proof. Other directories read from it, so this one comes first. |
| PulseMCP, https://www.pulsemcp.com | Its submit form (https://pulsemcp.com/submit) said submissions were paused; it ingests the official registry | Nothing extra once the registry entry exists. |
| Glama, https://glama.ai/mcp/servers | Builds its directory from submissions, automated discovery and the official registry; its connector pages use registry names (for example https://glama.ai/mcp/connectors/com.remote-mcp.mcp/remote-mcp) | To claim the listing: a `/.well-known/glama.json` on the server's domain naming a maintainer email that matches a Glama account (hub.krate.tech, so a hub route). |
| awesome-remote-mcp-servers, https://github.com/jaw9c/awesome-remote-mcp-servers | Pull request adding a row | Name, category, URL, authentication (OAuth 2.0 per the MCP spec only) and example usage. Curated: it prefers servers maintained by the company behind them, production-ready, with a support channel. |
| RemoteMCPList, https://remotemcplist.com | GitHub issue form (https://github.com/remotemcplist/servers/issues/new?template=new-server.yml) or a pull request with a YAML file in `servers/` | Name, category, URL, authentication and a description. |
| awesome-mcp-servers (punkpeye), https://github.com/punkpeye/awesome-mcp-servers | Pull request | Takes only servers people install and run themselves; remote-only servers are sent to awesome-remote-mcp-servers instead (its CONTRIBUTING.md). Not for us as the hub stands. |
| mcpservers.org (wong2/awesome-mcp-servers), https://mcpservers.org/submit | Web form; the GitHub list takes no pull requests | The form's fields. |
| mcp.so, https://mcp.so/submit | Web form | A project name, a description and a GitHub repository URL (github.com/incyashraj/krate, or the plugin repo). |
| Smithery, https://smithery.ai | Smithery now says it is part of Arcade, and its publishing docs (smithery.ai/docs/build/publish) answered 404 on 2026-11-21 | Unclear until its docs are back: it has listed hosted servers by URL, needing streamable HTTP and OAuth that answers 401 with the resource metadata (the hub does both). |
| Claude's Connectors Directory | Being handled separately | |

The Claude Code plugin (github.com/incyashraj/krate-claude-plugin) already
carries the connector address in its `plugin.json`.
