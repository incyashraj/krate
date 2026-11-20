# Krate for Claude Code

Make, check and share Krate apps by asking Claude. A Krate app is one small
`.krate` file that runs natively on Mac, Windows and Linux, opens from a link
in any browser, and can reach only what it declares.

## Install

Install Krate once:

```sh
curl -fsSL https://krate.tech/install.sh | sh        # macOS and Linux
irm https://krate.tech/install.ps1 | iex             # Windows PowerShell
```

Then, in Claude Code:

```
/plugin install krate --marketplace incyashraj/krate
```

or in a terminal:

```sh
claude plugin marketplace add incyashraj/krate
claude plugin install krate@krate
```

## What it adds

- **The Krate MCP server** (`krate mcp`): Claude can run a component or a
  `.krate` inside Krate's sandbox and inspect a bundle without running it.
- **The make-krate-app skill**: Claude reads Krate's real API
  (`krate authoring-context`), starts from a working template, checks every
  change with `krate check-app`, declares only the permissions the app needs,
  and packages, signs and publishes it as one link.

Ask something like: "Make me a habit tracker as a Krate app and give me a link."

Docs: https://krate.tech/docs/ · Issues: https://github.com/incyashraj/krate/issues
