// The blocked page shown INSIDE Talos (server: unblock.rs). Source-text guards, like
// forbidden-ui.test.mjs: app.js is DOM-bound and imported by no test.
//
// MEASURED on the Windows VM (2026-10-29, a local stand-in proxy refusing framing): step 1 laid
// the proxy's page over the panel's frame, a maximise moved it with the frame, "Approve access"
// inside it set the approval and WebView2 handed the released installer to `on_download`,
// which cancelled it and showed unblocked.html; Retry closed the page and installed; Skip, held
// by the pause, closed it too.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const appjs = readFileSync(new URL("../public/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../public/index.html", import.meta.url), "utf8");

function fnBody(src, name) {
  const at = src.indexOf(`function ${name}(`);
  assert.notEqual(at, -1, `${name}() must exist`);
  const open = src.indexOf("{", at);
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(open, i + 1);
  }
  throw new Error(`unbalanced braces in ${name}()`);
}

test("step 1 opens the page inside Talos, over the panel's frame", () => {
  const at = appjs.indexOf("fbStepOpen.onclick");
  const step = appjs.slice(at, appjs.indexOf("};", at));
  assert.match(step, /openBlockedPage\(fbActiveUrl\)/);
  assert.doesNotMatch(step, /openForbidden\(/, "the browser is the fallback, not step 1");
  const open = fnBody(appjs, "openBlockedPage");
  assert.match(open, /type: "unblock-open", url, rect: fbFrameRect\(\)/);
  assert.match(html, /<div id="fb-frame">/);
});

test("the page follows the window and goes with the panel", () => {
  assert.match(appjs, /type: "unblock-move", rect: fbFrameRect\(\)/);
  const dismiss = fnBody(appjs, "dismissFbModal");
  assert.ok(dismiss.indexOf("closeBlockedPage()") < dismiss.indexOf('classList.remove("show")'),
    "the native page must not outlive the panel it sits in");
  assert.match(fnBody(appjs, "closeBlockedPage"), /type: "unblock-close"/);
});

test("no Talos window here: the browser opens, as before", () => {
  const at = appjs.indexOf('case "unblock-unavailable"');
  assert.notEqual(at, -1);
  const arm = appjs.slice(at, appjs.indexOf("break;", at));
  assert.match(arm, /openForbidden\(fbActiveUrl\)/);
  assert.match(arm, /classList\.remove\("embedded"\)/);
  // And the user can always choose it.
  const browser = appjs.slice(appjs.indexOf('getElementById("fb-embed-browser")'));
  assert.match(browser.slice(0, 300), /closeBlockedPage\(\);\s*openForbidden\(fbActiveUrl\)/);
});

test("embedded, the stepper gives way to the frame", () => {
  assert.match(html, /#forbidden\.embedded \.fb-steps/);
  assert.match(html, /#forbidden\.embedded #fb-embed \{ display:flex/);
  assert.match(html, /#fb-embed \{ display:none; \}/);
});
