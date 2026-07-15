// Tests for the 403 firewall detector — pure, manager-agnostic. winget and
// brew/curl phrase a corporate-firewall block differently; is403 must catch
// both, extractUrl must pull the blocked URL when the output carries one and
// return null when it doesn't (the graceful-degrade path). Run: deno task test
import { assertEquals } from "@std/assert";
import { extractUrl, is403 } from "../src/forbidden.ts";

// Real-shape winget output: a Downloading line, then the 403.
const wingetForbidden = `Found Git (Git.Git) Version 2.50.1
Downloading https://github.com/git-for-windows/git/releases/download/v2.50.1/Git-2.50.1-64-bit.exe
  ██████████████████████████████  0.00 B /  67.5 MB
An unexpected error occurred while executing the command:
Forbidden (403)`;

// Real-shape brew output: curl fails fetching a bottle from ghcr.io.
const brewForbidden =
  `==> Downloading https://ghcr.io/v2/homebrew/core/jq/blobs/sha256:abc123
curl: (22) The requested URL returned error: 403
Error: jq: Failed to download resource "jq"
Download failed: https://ghcr.io/v2/homebrew/core/jq/blobs/sha256:abc123`;

const cleanWinget = `Found Git (Git.Git) Version 2.50.1
Downloading https://github.com/git-for-windows/git/releases/download/v2.50.1/Git-2.50.1-64-bit.exe
Successfully installed`;

Deno.test("is403: true on winget Forbidden (403)", () => {
  assertEquals(is403(wingetForbidden), true);
});

Deno.test("is403: true on brew/curl 403", () => {
  assertEquals(is403(brewForbidden), true);
});

Deno.test("is403: false on clean output", () => {
  assertEquals(is403(cleanWinget), false);
});

Deno.test("extractUrl: pulls the winget Downloading URL", () => {
  assertEquals(
    extractUrl(wingetForbidden),
    "https://github.com/git-for-windows/git/releases/download/v2.50.1/Git-2.50.1-64-bit.exe",
  );
});

Deno.test("extractUrl: pulls a URL from the brew/curl output", () => {
  assertEquals(
    extractUrl(brewForbidden),
    "https://ghcr.io/v2/homebrew/core/jq/blobs/sha256:abc123",
  );
});

Deno.test("extractUrl: null when no URL present", () => {
  assertEquals(extractUrl("Forbidden (403)"), null);
});

// Real streamed pty output: winget glues an OSC progress escape (\x1b]9;4;3;0\x1b\)
// directly onto the URL with no separating space. \S+ used to swallow it, so the
// opened link 404'd. extractUrl must stop at the control char.
Deno.test("extractUrl: stops at a trailing OSC escape sequence (the 404 bug)", () => {
  const streamed =
    "Downloading https://github.com/astral-sh/uv/releases/download/0.11.29/uv-x86_64-pc-windows-msvc.zip\x1b]9;4;3;0\x1b\\\r\n   - \r\n";
  assertEquals(
    extractUrl(streamed),
    "https://github.com/astral-sh/uv/releases/download/0.11.29/uv-x86_64-pc-windows-msvc.zip",
  );
});

Deno.test("extractUrl: trims trailing sentence punctuation", () => {
  assertEquals(
    extractUrl("blocked: https://ghcr.io/v2/homebrew/core/jq."),
    "https://ghcr.io/v2/homebrew/core/jq",
  );
});
