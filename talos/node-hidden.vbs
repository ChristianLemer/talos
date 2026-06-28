' hidden-start.vbs - run a command line with NO visible console window.
' Windows has no native "launch hidden" for cmd; this is the standard shim.
' Arg 0 = the command line to run.
' Why: the user opens the panel; node must run in the background with no black
' console box lingering behind the Edge app window.
CreateObject("WScript.Shell").Run WScript.Arguments(0), 0, False
