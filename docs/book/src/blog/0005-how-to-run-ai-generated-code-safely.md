# How to run AI-generated code without reading it

**Published:** 2026-08-05

An AI writes you a script. It is four hundred lines. You did not read it, and
you are not going to.

This is now the normal case, and the honest advice people give -- "always
review AI-generated code" -- is advice almost nobody follows for a small tool
they asked for in one sentence. Worth taking seriously instead: what protects
you when review does not happen?

## Why "trust the author" stopped working

Software distribution was built on identity. A signature says who made this,
and if it does something bad you know whom to blame. That works when a person
wrote the code and staked their reputation on it.

It does not survive AI authorship. Signing an app you did not read means
vouching for something you have not seen. The signature is still real; the
assurance behind it is gone.

The alternative is not better identity. It is making the question irrelevant by
limiting what the code can reach.

## Three levels of protection, and what each actually gives you

**Read it yourself.** Complete, and almost never happens. Four hundred lines of
unfamiliar Rust is an hour of careful work to save five minutes of typing.

**Run it in a container or a VM.** This is what most AI sandboxes do, and it
works, but it is coarse: the code gets a whole simulated computer with a whole
filesystem and usually a whole network stack. You have moved the blast radius,
not removed it. It is also heavy -- a VM per script is not something you do on a
laptop for a tip calculator.

**Give it only what it needs.** Capability-based security: the code starts with
nothing and receives specific, named permissions. Not "a filesystem" but "this
one folder". Not "the network" but "this one host on this one port". Anything
not granted does not exist -- calls to it fail, because there is no handle to
call through.

The third is the only one that scales down to a small app on your own machine.

## What this looks like in practice

Here is a real app's permission list, written by the app itself:

```
ui.window:create              Open the news reader window
io.stdout                     Report article counts for automated checks
net.connect:hnrss.org:443     Live open-source headlines via HN RSS
```

Four things, one of them a specific host on a specific port. That app cannot
read your documents. Not "should not" -- cannot. There is no filesystem handle
in its world to read them with.

This is checkable before you run anything. The declaration ships inside the
file, so you can inspect it without executing a single instruction.

## The questions worth asking about any sandbox

If you are evaluating something that claims to run untrusted code safely:

1. **What does the code get by default?** If the answer is "a filesystem" or "a
   network", that is a container, and the granularity is coarse.
2. **Can you see what it will ask for before running it?** If the first time
   you learn is when it does something, the check is theatre.
3. **Is the boundary enforced, or advisory?** A README saying an app only reads
   one folder is not a boundary.
4. **Does it run on my machine or someone else's?** Every mainstream AI sandbox
   in 2026 is cloud-hosted. That is fine for agents and wrong for a tool you
   want to use offline.

## Where Krate sits

[Krate](https://krate.tech) is a capability sandbox for desktop apps that runs
locally. An app is a WebAssembly component that can only call interfaces the
runtime hands it, and the runtime hands it exactly what you approved.

The permission list is shown before anything runs, in the app's own plain
words. Anything the app did not ask for is unreachable rather than discouraged.
And it is one file, so inspecting what an app wants is `krate run app.krate
--dump-caps` -- which reads the declaration without executing anything.

None of this makes reading the code pointless. It makes not reading it
survivable, which is the situation almost everyone is actually in.
