# Let your AI build Krate apps for you

If you use Claude Desktop or Cursor, you can describe an app in the chat you
already have open and get a working file back. No commands, no config files.

## Set it up

One command:

```
krate connect
```

It finds Claude Desktop or Cursor on your computer, shows you the one line it
wants to change, and asks before changing anything. If you have both, it will
tell you how to pick.

Then restart the app, and ask it something like:

> Build me a habit tracker that shows a weekly grid and remembers my streaks.
> Package it as a .krate.

That is the whole setup.

## What happens when you ask

1. It reads Krate's API reference, so it writes against the real thing rather
   than guessing.
2. It builds the app. **This takes five to twelve minutes.** It will tell you what
   stage it is at.
3. It checks the result: the app has to build, stay inside the sandbox, run,
   and draw its first screen.
4. You get a `.krate` file. Send it to anyone. They double-click it.

Everything happens on your computer. Krate never sees your AI account, and your
code never leaves your machine.

## If you ask for something Krate cannot do

You find out in about a second, with the reason and the nearest thing that
would work:

> Krate cannot build that. A Krate app runs in a sandbox and cannot read the
> apps or libraries already on your computer.
>
> Ask for this instead: a mail-reader interface with sample messages built in.

Krate would rather say no quickly than spend five minutes building something
that looks right and cannot do what you asked.

## Doing it by hand

`krate connect` writes a small block into a config file. If you would rather
place it yourself, or you use a different app, this is the block:

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

Use the full path to `krate` if a bare `krate` is not on your PATH inside the
app. `which krate` on macOS or Linux, `where krate` on Windows.

The files live at:

- **Claude Desktop, macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Claude Desktop, Windows**: `%APPDATA%\Claude\claude_desktop_config.json`
- **Cursor**: `~/.cursor/mcp.json`, or `.cursor/mcp.json` inside one project

In Claude Desktop you can also open it from the app: Claude menu, Settings,
Developer, Edit Config.

## Two other ways, if you would rather not connect anything

- **Paste one prompt into any chat.** Works in ChatGPT, Claude, Cursor, anything.
  https://krate.tech/docs/pages/krate-mode.html
- **One command in a terminal.**
  `krate create "your app" --output app.krate --agent claude`

## Something not working?

- **`krate connect` says it cannot find either app** -- install Claude Desktop
  or Cursor, sign in, and run it again.
- **The AI says it has no Krate tools** -- restart the app fully. Claude Desktop
  needs a real quit, not just closing the window.
- **It says your AI is not signed in** -- run `krate ai` to see what Krate can
  find on your machine.
- **`krate mcp` sits there doing nothing when you run it** -- that is correct.
  It is a background service that your AI app starts. You never run it yourself.
