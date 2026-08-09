# talos — admin commands
#
# Gestures for working ON Talos: diagnosing a machine, reporting a problem.
# Read-only unless a gesture says otherwise.
#
# Install (in your nu config or one-off):
#   use /path/to/talos/admin
#
# Works from a checkout on either platform — Windows included, which is the
# point: the answers that matter most come from a machine you are not sitting at.
#
# Common commands:
#   admin doctor                          # what does this machine actually answer?
#   admin doctor --deep                   # ...and probe per package (slower)
#   admin doctor --save report.yaml       # a report to attach to an issue
#   admin doctor capture --package Git.Git # raw output for the row that was wrong
#   admin doctor plugin-drift | where verdict == "outdated"
#
# Every gesture returns a TABLE, so filter and sort rather than read.
# Run `help admin doctor` for the full command list.

export use doctor
