// win-console.ts — kill the Windows console-flash storm at the source.
//
// THE BUG: in a --no-terminal (GUI subsystem) build the process has NO console.
// Every child process we spawn (Deno.Command) that is a console program —
// powershell for the 13 detection probes, netstat/taskkill for self-heal — makes
// Windows ALLOCATE A FRESH CONSOLE for it, which flashes a black window open and
// shut. 13+ flashes at startup = "the catastrophe". Deno gives us no per-child
// escape hatch (no windowsHide / CREATE_NO_WINDOW — only windowsRawArguments), so
// the fix can't live on the spawn side.
//
// THE FIX: give OUR process one console and HIDE it. Children then INHERIT the
// parent's (hidden) console instead of allocating their own — so no child ever
// shows a window. This also silences the self-heal flashes, for free.
//
// Honest limit: this is not zero-flash. Allocating our own console can show a
// brief blink before ShowWindow hides it (one flash, once, at startup) — but it
// trades 13+ child flashes for at most 1 parent flash. True zero would need
// CREATE_NO_WINDOW per child, which Deno does not expose.
//
// Two cases, handled correctly:
//   - We ALREADY have a console (debug build without --no-terminal, or launched
//     from a terminal): DO NOTHING — hiding it would swallow the logs that build
//     exists to show.
//   - We have NO console (the GUI --no-terminal build): AllocConsole, then hide
//     it, so children inherit a hidden console.
//
// Mac/Linux: strict no-op (there is no Win32 console model).

// Win32 calls, via kernel32/user32. Signatures kept minimal.
//   GetConsoleWindow() → HWND (0 if none)   [kernel32]
//   AllocConsole()     → BOOL               [kernel32]
//   ShowWindow(hwnd, nCmdShow) → BOOL       [user32]   SW_HIDE = 0
export function hideConsoleIfHeadless(
  log: (msg: string) => void = () => {},
): void {
  if (Deno.build.os !== "windows") return; // no console model off Windows

  try {
    const k = Deno.dlopen("kernel32.dll", {
      GetConsoleWindow: { parameters: [], result: "pointer" },
      AllocConsole: { parameters: [], result: "i32" },
    });
    const u = Deno.dlopen("user32.dll", {
      ShowWindow: { parameters: ["pointer", "i32"], result: "i32" },
    });

    const SW_HIDE = 0;
    let hwnd = k.symbols.GetConsoleWindow();

    if (hwnd !== null) {
      // A console already exists (debug build / launched from a terminal). Leave
      // it visible — its logs are the whole point of that build.
      log("win-console: console already present — leaving it visible");
      k.close();
      u.close();
      return;
    }

    // No console (GUI --no-terminal build): make one and hide it, so every child
    // process inherits a hidden console instead of popping its own window.
    if (k.symbols.AllocConsole() === 0) {
      log("win-console: AllocConsole failed — children may still flash");
      k.close();
      u.close();
      return;
    }
    hwnd = k.symbols.GetConsoleWindow();
    if (hwnd !== null) {
      u.symbols.ShowWindow(hwnd, SW_HIDE);
      log(
        "win-console: allocated + hidden — children inherit a hidden console",
      );
    }
    k.close();
    u.close();
  } catch (e) {
    // FFI can throw (perms, missing symbol). Never fatal — worst case is the
    // pre-existing flash behaviour, not a crash.
    log(`win-console: setup failed (continuing): ${(e as Error).message}`);
  }
}
