' Start.vbs - the single thing the user clicks.
'
' Runs app\start.bat with NO console window. start.bat does the work: check
' Node, launch the hidden server, open the panel. The only window the user
' sees in normal use is the panel itself. (Cold first-run shows a small
' "Installing Node" window on purpose - see start.bat.)
Dim shell, here
Set shell = CreateObject("WScript.Shell")
here = Left(WScript.ScriptFullName, InStrRev(WScript.ScriptFullName, "\"))
shell.Run """" & here & "talos\start.bat""", 0, False
