// The 403 panel: the firewall's page shown INSIDE Talos (server: unblock.rs), at once, and an
// automatic retry once it lets the download through. Source-text guards, like
// forbidden-ui.test.mjs: app.js is DOM-bound and imported by no test.
//
// MEASURED on the Windows VM (2026-10-29, a local stand-in proxy refusing framing): the page sat
// in the panel's frame and followed a maximise, "Approve access" inside it set the approval and
// WebView2 handed the released installer to `on_download`, which cancelled it; Retry installed;
// Skip, held by the pause, closed the page.
//
// One panel, no steps (operator, 2026-10-29): a 403 only happens on Windows, where the page
// opens inside; the approval is the one thing to do, and the retry follows by itself.
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
function arm(name) {
  const at = appjs.indexOf(`case "${name}":`);
  assert.notEqual(at, -1, `the front no longer handles \`${name}\``);
  return appjs.slice(at, appjs.indexOf("break;", at));
}

test("a block opens the firewall's page at once, inside the panel — no step to click", () => {
  assert.match(arm("forbidden"), /showForbidden\(msg\.i, msg\.url \|\| null, msg\.inside === true\)/);
  assert.match(fnBody(appjs, "showFbPanel"), /if \(inside && url\) openBlockedPage\(url\)/);
  assert.match(fnBody(appjs, "showForbidden"), /showFbPanel\(i, url, inside\)/);
  assert.match(fnBody(appjs, "openBlockedPage"), /type: "unblock-open", url, rect: fbFrameRect\(\)/);
  // The 1-2-3 stepper is gone, with its markup.
  for (const gone of ['id="fb-open"', 'id="fb-retry"', "fb-steps", "fb-moreinfo"]) {
    assert.ok(!html.includes(gone), `${gone} is still in index.html`);
  }
  assert.doesNotMatch(appjs, /setFbStep|fbStepOpen|fbStepRetry/);
});

test("the approval seen, Talos counts down and retries by itself — cancellable", () => {
  const ev = arm("unblock-event");
  assert.match(ev, /msg\.event === "released"\) startRetryCountdown\(\)/);
  assert.match(ev, /msg\.event === "cancel"\) stopRetryCountdown\(true\)/);
  assert.match(ev, /msg\.event === "retry" && fbActive !== null\) retryStep\(fbActive\)/);
  const count = fnBody(appjs, "startRetryCountdown");
  assert.match(count, /fbEmbedRetry\.hidden\) return/, "never past the retry cap");
  assert.match(count, /retryStep\(i\)/);
  assert.match(count, /fbRetryWhen\.textContent = `in \$\{left\} s`/, "the Retry button says when");
  assert.match(fnBody(appjs, "sayFb"), /automatic retry is off/);
  assert.match(appjs, /const FB_COUNTDOWN = 5;/);
  // Whatever takes the page away stops the count.
  assert.match(fnBody(appjs, "closeBlockedPage"), /stopRetryCountdown\(\)/);
});

test("the count lives in Talos's page inside the frame, with its own buttons", () => {
  const page = readFileSync(new URL("../public/unblocked.html", import.meta.url), "utf8");
  assert.match(page, /const SECONDS = 5;/, "the same count as the panel's FB_COUNTDOWN");
  assert.match(page, /fetch\("\/unblock\/act", \{ method: "POST", body: word \}\)/);
  assert.match(page, /id="now">Retry now</);
  assert.match(page, /id="cancel">Cancel</);
  // At the retry cap the page promises nothing (review): the server sends it `?left=0`.
  assert.match(page, /get\("left"\) === "0"/);
  assert.match(page, /querySelector\("\.actions"\)\.hidden = true/);
  assert.match(page, /\[hidden\] \{ display:none !important; \}/, "or the flex row ignores hidden");
  // Talos's blue and buttons, not a page of its own style.
  assert.match(page, /--head:#7aa2f7/);
  assert.match(page, /\.primary \{ background:var\(--fill\)/);
  // And the panel's Retry is Talos's two-line blue button, like Apply.
  assert.match(html, /id="fb-embed-retry" class="applybtn live"><span>Retry<\/span><small id="fb-retry-when">/);
});

test("the page follows its frame and goes with the panel", () => {
  assert.match(appjs, /type: "unblock-move", rect: fbFrameRect\(\)/);
  assert.match(appjs, /new ResizeObserver\(placeBlockedPage\)\.observe\(fbFrame\)/);
  assert.match(fnBody(appjs, "watchScale"), /resolution: \$\{devicePixelRatio\}dppx/);
  const dismiss = fnBody(appjs, "dismissFbModal");
  assert.ok(dismiss.indexOf("closeBlockedPage()") < dismiss.indexOf('classList.remove("show")'),
    "the native page must not outlive the panel it sits in");
  assert.match(fnBody(appjs, "closeBlockedPage"), /type: "unblock-close"/);
});

test("a page that cannot open inside leaves the panel small, the browser offered — never opened", () => {
  const unavailable = arm("unblock-unavailable");
  assert.match(unavailable, /classList\.remove\("embedded"\)/);
  assert.doesNotMatch(unavailable, /openForbidden\(/, "the browser opens on a click only");
  assert.match(html, /#fb-frame \{ display:none; \}/);
  assert.match(html, /#forbidden\.embedded #fb-frame \{ display:flex/);
  const browser = appjs.slice(appjs.indexOf("fbBrowserBtn.onclick"));
  assert.match(browser.slice(0, 120), /closeBlockedPage\(\);\s*openForbidden\(fbActiveUrl\)/);
});

test("a new block takes away a page shown for another one", () => {
  const show = fnBody(appjs, "showForbidden");
  assert.ok(show.indexOf("closeBlockedPage()") < show.indexOf("showFbPanel("));
});

test("at the retry cap the panel's Retry goes", () => {
  assert.match(fnBody(appjs, "paintFbAttempt"), /fbEmbedRetry\.hidden = !canRetry/);
  assert.match(fnBody(appjs, "showForbidden"), /fbEmbedRetry\.hidden = false/);
});

test("review: no countdown on a page nobody approved, none promised at the cap, none on another row", () => {
  // At the cap the line stops saying "Talos retries by itself".
  assert.match(fnBody(appjs, "sayFb"), /fbEmbedRetry\.hidden/);
  assert.match(fnBody(appjs, "paintFbAttempt"), /sayFb\(\)/);
  // Switching rows stops the other row's countdown.
  assert.match(fnBody(appjs, "showFbPanel"), /if \(fbActive !== i\) closeBlockedPage\(\)/);
});
