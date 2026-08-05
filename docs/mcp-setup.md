# Connect Krate to Claude Desktop or Cursor

Krate ships an MCP server. Once it is connected, you describe an app in chat and
your AI builds it, checks it, and hands you the `.krate` file. No commands.

## Before you start

You need Krate installed and an AI coding tool signed in. Check both:

```
krate ai
```

That prints which AI tools are on your machine. If none are, install one and
sign in to it, then run it again.

The build runs on your computer, not on ours. Your code never leaves your
machine, and Krate never sees your AI account.

## Claude Desktop

The reliable way to find the config file is from inside the app: Claude menu ->
Settings -> Developer -> Edit Config. That creates the file if it does not
exist yet.

It lives here:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

Claude Desktop is published for macOS and Windows. If you are on Linux and
using a community build, use its own config location -- we have not verified
one, so trust the app's Edit Config button over any path written here.

Add Krate under `mcpServers`. If the file is empty or does not exist, this is
the whole file:

```json
{
  "mcpServers": {
    "krate": {
      "command": "krate",
      "args": ["mcp"]
    }
  }
}
```

If `krate` is not on your PATH inside Claude Desktop, use the full path instead.
`which krate` on macOS or Linux, `where krate` on Windows, will tell you what to
put there.

Restart Claude Desktop. Krate's tools appear in the tools menu.

## Cursor

Create or edit `.cursor/mcp.json` in your project, or `~/.cursor/mcp.json` to
have it everywhere:

```json
{
  "mcpServers": {
    "krate": {
      "command": "krate",
      "args": ["mcp"]
    }
  }
}
```

Reload Cursor.

## Check it worked

Ask your AI:

> What Krate tools do you have?

Seven of them build apps: `krate_schema`, `krate_examples`, `krate_start_build`,
`krate_build_status`, `krate_check`, `krate_package`, and `krate_run`.

Two more, `inspect_bundle` and `run_component`, are for opening a `.krate`
somebody sent you rather than making one. Nine in total is correct.

## Then just talk

> Build me a habit tracker that shows a weekly grid and remembers my streaks.
> Package it as a .krate.

What happens next, so nothing is a surprise:

1. The model reads Krate's API reference, so it writes against the real thing
   rather than guessing.
2. It starts a build and gets a job id back straight away. **A build takes two
   to five minutes.** The model polls and can tell you what stage it is at.
3. It checks the result with the same six-stage oracle the command line uses:
   the app has to build, import nothing outside `krate:*`, run, and paint a
   frame.
4. You get a `.krate` file. Send it to anyone; they double-click it.

## If you ask for something Krate cannot do

You get told in about a second, with the reason and the nearest thing that would
work:

> Krate cannot build that. A Krate app runs in a sandbox and cannot read the
> apps or libraries already on your computer.
>
> Ask for this instead: a mail-reader interface with sample messages built in.

This is deliberate. Krate would rather refuse in a second than spend five
minutes producing an app that looks right and cannot do what you asked.

## Doing it without MCP

Two other ways, if you would rather not connect anything:

- **Krate Mode** -- paste one prompt into any chat and it writes correct Krate
  code, which you then build locally. https://krate.tech/docs/pages/krate-mode.html
- **The command line** -- `krate create "your app" --output app.krate --agent claude`.
  There is a builder that writes the command for you at
  https://krate.tech/docs/pages/make-an-app-with-ai.html
