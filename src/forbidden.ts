// src/forbidden.ts — the 403 firewall detector. PURE, manager-agnostic: no IO,
// no state — just two functions over captured/streamed command output. On
// corporate-managed machines the corporate firewall returns 403 Forbidden on GitHub
// downloads; winget phrases it "Forbidden (403)", brew shells out to curl which
// says "curl: (22) … error: 403" / "Download failed". We match BOTH, broadly:
// a false positive costs only a harmless extra Retry offer, a false negative
// leaves the user stranded with a cryptic failure — so we err toward matching.
//
// Sibling of watch-window.ts (the waiting-window watcher): both watch a running
// step and surface a recoverable condition; this one keys off the OUTPUT text
// rather than a hidden window. server.ts taps is403 in runInPty's read loop for
// the live banner and re-scans on exit for the verdict.

// Does this output (stdout+stderr, streamed chunk or whole buffer) show a
// corporate-firewall 403? Deliberately broad — see file header.
export function is403(text: string): boolean {
  return /Forbidden \(403\)/i.test(text) || // winget
    /\b403\b/.test(text) && // brew/curl: a bare 403 near a download failure
      (/curl:\s*\(22\)/i.test(text) || /Download failed/i.test(text) ||
        /returned error/i.test(text));
}

// Pull the blocked URL out of the output so the caller can open it for the user.
// winget prints "Downloading https://…" before the 403; brew prints an "==>
// Downloading https://…" line and often a "Download failed: https://…" line.
// Returns the FIRST http(s) URL found, or null when none is present (the
// graceful-degrade path: banner without a link, no auto-open).
export function extractUrl(text: string): string | null {
  // \S+ is too greedy on STREAMED pty output: winget appends an OSC progress
  // escape sequence (…\x1b]9;4;3;0\x1b\) directly after the URL with no space,
  // so \S+ swallows the escape bytes and the opened link 404s. Stop at any
  // control char (incl. ESC 0x1b), then trim trailing punctuation a URL can't
  // really end on (the curl line sometimes ends "…403." etc.).
  const m = text.match(/https?:\/\/[^\s\x00-\x1f\x7f]+/);
  if (!m) return null;
  return m[0].replace(/[.,;:!?)\]}>'"]+$/, "");
}
