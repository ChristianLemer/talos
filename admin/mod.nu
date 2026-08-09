# talos — admin commands
#
# Gestures for working ON Talos: diagnosing a machine, reporting a problem.
# Read-only unless a gesture says otherwise.
#
# Install (in your nu config or one-off):
#   use /path/to/talos/admin
#
# Common commands:
#   admin doctor              # what does this machine actually answer?
#   admin doctor --deep       # ...and time the per-package probes
#   admin doctor --save       # write a timestamped report to attach to an issue
#
# Run `help admin` for the full command list.

export use doctor
