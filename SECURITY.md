# Security

Talos runs commands on the machine it is launched on — installers, package
managers, shell probes. Treat a report about it the way you would treat one about
an installer.

**Report privately** to christian@lemer.be. Say what you observed, on which OS,
with which release (the window title shows the tag). You will get an answer; a fix
lands in the next release and is named in its notes.

What is and is not in scope, so a report lands well:

- **In scope**: a command Talos runs that does more than its row says; content
  (`catalog/`, `bundles/`) that can make the engine run something the author of
  that content did not write; a path or a shell line built outside
  `platform::shell_probe` / `pty_shell`.
- **Known, by design**: the binaries are **unsigned** for now — Gatekeeper and
  SmartScreen will warn. Every command Talos runs is shown live in the row's
  terminal, and `Talos --check` refuses content the runtime would silently skip.
