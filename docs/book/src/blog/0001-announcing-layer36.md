# Announcing Layer36

**Status:** Draft
**Target:** Refresh before public launch

Layer36 is an experiment in making native application development portable again.
The goal is direct: write an app once, package it once, and run it natively on
the devices people actually use.

The platform is built around WebAssembly and the Component Model. Applications
compile to a portable component, call a Universal API defined in WIT, and run
inside a host runtime that enforces capability-based permissions before mapping
those calls onto native operating-system APIs.

This is not a new kernel and it is not an emulator. It is a meta-platform: a
runtime, standard library, permission model, bundle format, and distribution
layer that sit above existing operating systems.

Layer36 is still pre-alpha, but the repo is now past the original foundation
work. Phase 1 proved the base runtime path. Phase 2 is building the first
useful app-platform slice: UAPI modules for files, network, time, locale, and
I/O, plus manifest-declared capabilities and runtime permission checks.

The current milestone is Phase 2 exit: freeze the first UAPI contract, collect
clean cross-host evidence, and complete an outside developer walkthrough before
starting the desktop UI phase.

Follow the roadmap in the docs, read the build plan, and open an issue if there
is a small piece you want to help with.
