// app.js — the Talos panel UI. Pure decision logic lives in decision.js
// (imported below), shared with the server so the rule can never drift.
import { postureDefault as _postureDefault } from "./decision.js";
import * as M from "./model.js";
const model = M.createModel();

const stepsEl = document.getElementById("steps");
const overall = document.getElementById("overall");
const b64dec = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const dec = new TextDecoder();
const rows = {}; // i → { details, badge, statusLabel, host, term }

// --- startup splash: rotating quips while we check the machine ---
const QUIPS = [
  "Saddling up the centaur…",
  "Half human, half horse, all business.",
  "Teaching your machine to talk to the AI…",
  "Counting hooves… four, good.",
  "Polishing the bronze guardian…",
  "Checking what's already on board…",
  "Bow drawn, aiming at the dependencies…",
  "No terminals were harmed in this setup.",
  "Galloping through your PATH…",
  "Brewing something better than coffee…",
  // geekier batch
  "sudo make me a sandwich…",
  "Resolving dependencies (it's npm all the way down)…",
  "Reticulating splines… wait, wrong installer.",
  "git blame says it was working yesterday.",
  "Spawning a pty, fork() and pray…",
  "rm -rf /doubts…",
  "Compiling… perfect time for a swordfight.",
  "It's not a bug, it's an undocumented hoof.",
  "while (!asleep) { installDependencies(); }",
  "Exit code 0 — the sweetest two characters.",
];
const splash = document.getElementById("splash");
const quipEl = document.getElementById("splash-quip");
let qi = Math.floor(Date.now() % QUIPS.length);
quipEl.textContent = QUIPS[qi];
const quipTimer = setInterval(() => {
  quipEl.style.opacity = "0";
  setTimeout(() => {
    qi = (qi + 1) % QUIPS.length;
    quipEl.textContent = QUIPS[qi];
    quipEl.style.opacity = "1";
  }, 300);
}, 2200);
let splashDone = false;
function hideSplash() {
  if (splashDone) return;
  splashDone = true;
  clearInterval(quipTimer);
  // keep it up a beat so the centaur is seen even if detection is instant
  setTimeout(() => {
    splash.classList.add("hide");
    setTimeout(() => splash.remove(), 450);
  }, 400);
}
setTimeout(hideSplash, 12000); // safety: never let the splash stick forever

// Shorter terminals (10 rows) — they scroll, and tall fixed boxes waste
// vertical space on small screens. Smaller font on short viewports.
const TERM_ROWS = window.innerHeight < 700 ? 8 : 12;
const newTerm = () =>
  new Terminal({
    cols: 100,
    rows: TERM_ROWS,
    fontSize: window.innerHeight < 700 ? 11 : 12,
    convertEol: false,
    theme: { background: "#1a1b26", foreground: "#c0caf5" },
  });

const GLYPH = {
  waiting: "·",
  installing: "⠹",
  uninstalling: "⠹",
  upgrading: "⠹",
  ok: "✓",
  absent: "·",
  fail: "✗",
  unknown: "?",
};
const LABEL = {
  waiting: "absent",
  installing: "installing…",
  uninstalling: "removing…",
  upgrading: "updating…",
  ok: "present",
  absent: "removed",
  fail: "failed",
  // indeterminate: no practicable route to constate presence on this machine
  // (e.g. a winget-only package on Mac). NOT "absent" — we genuinely can't know.
  unknown: "—",
};

// TWO LAYERS (chezmoi convention).
//  1. POSTURE — the author's policy, declared in bundle.yaml, per package:
//       mandatory  → always present, user CANNOT decline (locked pill)
//       opt-out    → present by default, user MAY remove
//       opt-in     → absent by default, user MAY add
//       forbidden  → always absent (removed), user CANNOT enable (locked)
//  2. OVERRIDE — the user's pill, only meaningful for opt-in/opt-out:
//       "auto" → follow the posture's default
//       "on"   → I want it present   (opt-in / opt-out only)
//       "off"  → I want it absent     (opt-in / opt-out only)
// The DESIRED state of every package is resolved from posture + override
// (see desiredState). Apply converges the machine to the desired state —
// model A, chezmoi-pure: an opt-in left auto but present WILL be removed.
// Only "on"/"off" overrides are persisted; "auto" is absence.
const bundleEls = {}; // bundle name → DOM refs + ephemeral run counters
// (data — pkgIds/posture/selectable — lives in model.bundles).
const ICON_COPY =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>';
const ICON_OK =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';

// --- persistence: only USER-MOVED toggles are saved (untouched = absence).
// Saved by NAME (bundle::package) so it survives bundles being added/reordered.
function persistSelection() {
  ws.send(JSON.stringify({
    type: "set-selection",
    selection: { pkgs: M.persistablePkgs(model) },
  }));
}
function applySavedSelection(sel) {
  M.applySavedSelection(model, sel);
  for (const i of model.pkgs.keys()) paintPkg(i);
  for (const name of Object.keys(bundleEls)) paintBundle(name);
  refreshLiveness();
}

// A real sliding switch: a track with a knob that sits left (out), right (in),
// or centre (mixed). State classes (on-in / on-out / mixed / locked / deviated)
// are set by paintPkg/paintBundle; the CSS positions and colours the knob.
function makeToggle() {
  const el = document.createElement("span");
  el.className = "toggle on-in";
  el.innerHTML = '<span class="knob"></span>';
  return el;
}

// Colour rule (ONE rule, same in simple and advanced, package and bundle): a
// switch goes VIVID only when a plain Apply would CHANGE the machine — green if
// it would install/update, red if it would remove. An on switch that's already
// installed stays calm (nothing to do). This mirrors the row plan-borders, and
// avoids the "everything is green" wall. The default-side dot stays advanced-only.
function actClass(i) {
  const a = M.actionOf(model, i);
  if (a === "install" || a === "upgrade") return " act-add";
  if (a === "uninstall") return " act-remove";
  return "";
}
function paintPkg(i) {
  const r = rows[i];
  if (!r) return;
  const posture = M.postureOf(model, i);
  const on = M.toggleOf(model, i); // "in" | "out"
  const locked = M.isLocked(model, i);
  const deviated = M.isDeviated(model, i);
  const el = r.chk;
  el.className = "toggle" + (on === "in" ? " on-in" : " on-out") +
    (locked ? " locked" : "") + (deviated ? " deviated" : "") + actClass(i);
  el.dataset.default = _postureDefault(posture); // CSS marks the default side (advanced only)
  el.style.cursor = locked ? "not-allowed" : "pointer";
  el.title = locked
    ? (posture === "mandatory"
      ? "Required by the author — always installed"
      : "Blocked by the author — never installed")
    : `Toggle: in = install, out = skip (author default: ${
      _postureDefault(posture)
    })`;
  // A row whose desired state is absent reads dimmer (not wanted).
  r.details.classList.toggle("row-out", M.desiredOf(model, i) === "absent");
}
function setToggle(i, state, opts = {}) {
  if (!M.setDecision(model, i, state)) return; // locked → refused
  paintPkg(i);
  const b = model.pkgs.get(i)?.bundle; // repaint the parent bundle pill:
  if (b) paintBundle(b); // its in/out/MIXED may have changed
  refreshLiveness();
  if (!opts.silent) persistSelection();
}
// Clicking the toggle flips to the OTHER side (in ↔ out) from wherever it is now.
function flipPkg(i) {
  if (!M.isLocked(model, i)) {
    setToggle(i, M.toggleOf(model, i) === "in" ? "out" : "in");
  }
}

// A bundle is LOCKED if not selectable, or all its packages are locked.
function bundleLocked(name) {
  return M.bundleLocked(model, name);
}
// The bundle toggle APPLIES a side to all its changeable packages at once (not an
// inherited layer — it writes each package's own toggle). Locked packages keep
// the author's choice.
function setBundleToggle(name, state, opts = {}) {
  if (bundleLocked(name)) return;
  const bm = model.bundles.get(name);
  if (!bm) return;
  bm.pkgIds.forEach((i) => {
    if (!M.isLocked(model, i)) setToggle(i, state, { silent: true });
  });
  paintBundle(name);
  if (!opts.silent) persistSelection();
}
// Clicking the bundle toggle: if every changeable package is already "in", flip
// them all to "out"; otherwise pull them all "in". (Majority-in → out, else in.)
function flipBundle(name) {
  if (bundleLocked(name)) return;
  const bm = model.bundles.get(name);
  const free = bm.pkgIds.filter((i) => !M.isLocked(model, i));
  const allIn = free.every((i) => M.toggleOf(model, i) === "in");
  setBundleToggle(name, allIn ? "out" : "in");
}
// Paint the bundle's own toggle, reflecting its changeable packages in THREE
// states: all in → on-in, all out → on-out, a MIX → "mixed" (greyed, neither
// side lit) so the bundle toggle never lies about a panachage. deviated = any
// package moved off its author default.
function paintBundle(name) {
  const be = bundleEls[name];
  const bm = model.bundles.get(name);
  if (!be || !bm) return;
  const el = be.chk;
  if (bundleLocked(name)) {
    el.className = "toggle locked " +
      (bm.posture === "forbidden" ? "on-out" : "on-in");
    el.dataset.default = _postureDefault(bm.posture);
    el.style.cursor = "not-allowed";
    el.title = bm.selectable
      ? "All packages here are fixed by the author"
      : "This bundle is always on";
    return;
  }
  const state = M.bundleToggleState(model, name); // "on-in" | "on-out" | "mixed"
  const deviated = bm.pkgIds.some((i) => M.isDeviated(model, i));
  const actDir = M.bundleAct(model, name); // "add" | "remove" | ""
  const act = actDir === "add"
    ? " act-add"
    : actDir === "remove"
    ? " act-remove"
    : "";
  el.className = "toggle " + state + (deviated ? " deviated" : "") + act;
  el.dataset.default = _postureDefault(bm.posture);
  el.style.cursor = "pointer";
  el.title = state === "mixed"
    ? "Mixed — some in, some out. Click to pull all in."
    : "Toggle the whole bundle in / out";
}
function refreshBundleChk(name) {
  paintBundle(name);
}

// Would applying package `i` actually DO something, given its EFFECTIVE
// decision? on & absent → install; on & present & outdated → upgrade;
// off & present → remove. auto → never (left as the machine has it).
// The action clicking a row's button does — it ALWAYS inverts the current
// state: absent → install, present → uninstall, present-but-stale → update
// (the useful move, not a pointless remove). null only when present and
// un-uninstallable. This is the row's immediate manual actuator.
function buttonAction(i) {
  return M.buttonAction(model, i);
}
// Would a plain Apply act here? Delegates to the SHARED rule (actionFor) — the
// exact same call the server makes — so the button's green/red preview always
// matches what Apply will really do. Lights the button; the label above still
// shows the manual invert action regardless.
function isActionable(i) {
  return M.isActionable(model, i);
}
// Light up only the Apply buttons that would do something; leave the rest as
// ghost outlines. The eye lands on what matters. Runs after any change to
// selection or detected state.
let applyRunning = false;
function refreshLiveness() {
  const ids = Object.keys(rows).map(Number);
  // Per-row buttons: light the ones that would act; all stay clickable at
  // hover for a manual re-run, but disabled while a run is on.
  for (const i of ids) {
    const r = rows[i];
    const act = buttonAction(i); // always the invert action (label)
    const inPlan = isActionable(i); // would a plain Apply act here?
    const show = !!act && !applyRunning;
    // Colour follows the PLAN: directional green/red only if Apply will act
    // now; otherwise blue (available manual escape hatch, no pending change).
    r.apply.classList.toggle("live", show && inPlan);
    r.apply.classList.toggle("add", show && inPlan && act.dir === "add");
    r.apply.classList.toggle("remove", show && inPlan && act.dir === "remove");
    r.apply.classList.toggle("avail", show && !inPlan);
    r.apply.textContent = act ? act.verb : "—"; // button IS the action; — if none possible
    r.apply.disabled = applyRunning || !act;
    // Tint the WHOLE row when it's in the plan — the change is unmissable, not
    // hidden in a small button. When in-plan, act.dir is the plan's direction.
    r.details.classList.toggle("plan-add", inPlan && act && act.dir === "add");
    r.details.classList.toggle(
      "plan-remove",
      inPlan && act && act.dir === "remove",
    );
    paintPkg(i); // switch colour follows the plan — refresh it as machine state lands
  }
  // Bundle buttons: live if any of their packages would act.
  for (const name of Object.keys(bundleEls)) {
    const be = bundleEls[name];
    const live = M.bundleAnyActionable(model, name);
    be.apply.classList.toggle("live", live && !applyRunning);
    be.apply.disabled = applyRunning;
    paintBundle(name); // bundle switch colour follows the plan too
    // Preview the plan AT REST: a bundle with pending actions opens, a stable
    // one folds. Only when idle — during a run the execution logic (setStatus)
    // owns open/close, and we never fight the user's manual toggle mid-run.
    if (!applyRunning) be.details.open = live;
  }
  // Global button: live if anything anywhere would act.
  const g = document.getElementById("install-all");
  g.classList.toggle("live", ids.some(isActionable) && !applyRunning);
  g.disabled = applyRunning;
  // Reset is available only if the user has moved something off the defaults.
  const deviated = [...model.pkgs.keys()].some((i) => M.isDeviated(model, i));
  const reset = document.getElementById("reset-all");
  if (reset) reset.disabled = applyRunning || !deviated;
}

// Apply. We send the DECIDED packages only, split into `on` (want present)
// and `off` (want absent), using each package's EFFECTIVE decision (its own,
// or inherited from its bundle). Everything else is auto → the server never
// touches it. `scope` (a per-bundle Apply) further restricts which indices
// may act. auto is the safety: no decision, no action.
function applyScoped(scopeIdx) {
  if (applyRunning) return;
  // Model A: EVERY package has a desired state (posture + override). Send it
  // all — on = want present, off = want absent. The server converges.
  const on = [], off = [];
  for (const i of Object.keys(rows).map(Number)) {
    if (M.desiredOf(model, i) === "present") on.push(i);
    else off.push(i);
  }
  applyRunning = true;
  refreshLiveness(); // lock every Apply button during the run
  overall.textContent = "applying…";
  // Focus mode engages on the server's `apply-plan` reply (it computes the plan),
  // not here — so we show the exact set of steps that will run.
  ws.send(JSON.stringify({ type: "apply", on, off, scope: scopeIdx }));
}

function render(bundles, steps) {
  M.loadPlan(model, bundles, steps);
  for (const b of bundles) {
    const bd = document.createElement("details");
    bd.className = "bundle";
    const bsum = document.createElement("summary");
    bsum.className = "bundle-head";
    const chk = makeToggle();
    chk.onclick = (e) => {
      e.preventDefault();
      e.stopPropagation(); // flip without folding
      flipBundle(b.name);
    };
    const ico = document.createElement("span");
    ico.className = "ico";
    ico.textContent = b.emoji || "📦";
    const ct = document.createElement("span");
    ct.className = "ct";
    ct.innerHTML = `<span class="cn">${b.name}</span><span class="cd">${
      b.description || ""
    }</span>`;
    const bst = document.createElement("span");
    bst.className = "bstatus";
    const bapply = document.createElement("button");
    bapply.className = "applybtn";
    bapply.textContent = "Apply";
    bapply.title = "Apply just this bundle";
    bapply.onclick = (e) => {
      e.preventDefault();
      e.stopPropagation(); // act without folding the accordion
      applyScoped(model.bundles.get(b.name)?.pkgIds);
    };
    bsum.append(chk, ico, ct, bst, bapply);
    const body = document.createElement("div");
    body.className = "bundle-body";
    bd.append(bsum, body);
    stepsEl.append(bd);
    bundleEls[b.name] = {
      details: bd,
      head: bsum,
      status: bst,
      chk,
      apply: bapply,
      active: 0, // ephemeral: packages running in this bundle this Apply (animation counter, not domain state, reset each run)
      failed: false, // ephemeral: a package here failed this run (drives the bundle status badge; not persisted)
    };
  }

  for (const s of steps) {
    const be = bundleEls[s.bundle];
    const body = be ? be.details.querySelector(".bundle-body") : stepsEl;
    const d = document.createElement("details");
    d.classList.add("posture-" + (s.posture || "mandatory")); // dim optional (opt-*) rows
    const sum = document.createElement("summary");
    const chk = makeToggle();
    chk.onclick = (e) => {
      e.preventDefault();
      e.stopPropagation();
      flipPkg(s.i);
    };
    const badge = document.createElement("span");
    badge.className = "badge waiting";
    badge.textContent = "·";
    const name = document.createElement("span");
    name.className = "name";
    name.innerHTML = `${s.name}` +
      (s.description ? ` <span class="desc">— ${s.description}</span>` : "");
    const delta = document.createElement("span"); // version delta on upgrade, e.g. 2.54 → 2.55
    delta.className = "verdelta";
    const st = document.createElement("span");
    st.className = "statusLabel waiting";
    st.textContent = "absent";
    const apply = document.createElement("button");
    apply.className = "rerun";
    apply.textContent = "apply";
    apply.title = "Apply this one now";
    apply.onclick = (e) => {
      e.preventDefault();
      if (applyRunning) return;
      const act = buttonAction(s.i); // send EXACTLY what the button shows
      if (!act) return;
      applyRunning = true;
      refreshLiveness();
      overall.textContent = "applying…";
      ws.send(JSON.stringify({ type: act.type, i: s.i }));
    };
    sum.append(chk, badge, name, delta, st, apply);
    const panel = document.createElement("div");
    panel.className = "panel";
    const copy = document.createElement("button");
    copy.className = "copy";
    copy.innerHTML = ICON_COPY;
    copy.title = "Copy this step's output";
    copy.onclick = (e) => {
      e.preventDefault();
      // Copy the log VERBATIM — only ANSI colour codes stripped (they're not
      // information, just noise in a paste). Everything else stays: the copy is
      // for debugging/sharing, so fidelity to what actually ran is the point.
      const text = (model.pkgs.get(s.i)?.log || "").replace(
        /\x1b\[[0-9;?]*[A-Za-z]/g,
        "",
      );
      navigator.clipboard.writeText(text).then(() => {
        copy.innerHTML = ICON_OK;
        setTimeout(() => copy.innerHTML = ICON_COPY, 1200);
      });
    };
    const host = document.createElement("div");
    host.className = "term-host";
    panel.append(copy, host);
    d.append(sum, panel);
    body.append(d);
    rows[s.i] = {
      details: d,
      chk,
      badge,
      statusLabel: st,
      delta,
      apply,
      host,
      term: null,
    };
    paintPkg(s.i); // initial pill: locked word for mandatory/forbidden, else auto
  }
  // Now that every bundle knows its packages, paint bundle pills (locked or auto).
  for (const name of Object.keys(bundleEls)) refreshBundleChk(name);
}

function ensureTerm(i) {
  const r = rows[i];
  if (!r.term) {
    r.term = newTerm();
    r.term.open(r.host);
  }
  return r.term;
}

const RUNNING = new Set(["installing", "uninstalling", "upgrading"]);

// --- Apply focus mode --------------------------------------------------------
// During an Apply the screen narrows to THE PLAN: every step that WILL run stays
// visible the whole time (so you see the full to-do list and follow progress down
// it), while everything NOT in the plan is hidden. Successes stay visible but
// fold; failures stay visible AND open. On done, everything reappears — the
// failures already open, standing out. The plan comes from the server's
// `apply-plan` message (it's the server that computes install/uninstall).
function enterFocusMode(planIndices) {
  document.body.classList.add("applying");
  const inPlan = new Set(planIndices);
  for (const i of Object.keys(rows).map(Number)) {
    // focus-show = this row is part of the plan → stays visible throughout.
    rows[i].details.classList.toggle("focus-show", inPlan.has(i));
  }
  recomputeEmptyBundles();
}
function exitFocusMode() {
  document.body.classList.remove("applying");
  // Reveal everything again; leave the per-row open/fold state as setStatus left
  // it (failures open, successes folded) — the failures thus stand out on return.
  for (const i of Object.keys(rows).map(Number)) {
    rows[i].details.classList.remove("focus-show");
  }
  for (const name of Object.keys(bundleEls)) {
    bundleEls[name].details.classList.remove("focus-empty");
  }
}
// Hide a bundle card whose every row is currently focus-hidden (no empty shells).
function recomputeEmptyBundles() {
  if (!document.body.classList.contains("applying")) return;
  for (const name of Object.keys(bundleEls)) {
    const be = bundleEls[name];
    const pkgIds = model.bundles.get(name)?.pkgIds || [];
    const anyShown = pkgIds.some((i) =>
      rows[i]?.details.classList.contains("focus-show")
    );
    be.details.classList.toggle("focus-empty", !anyShown);
  }
}

function setStatus(i, status) {
  const r = rows[i];
  if (!r) return;
  if (RUNNING.has(status)) {
    M.resetLog(model, i);
    if (r.term) r.term.reset();
  } // fresh run
  r.badge.className = "badge " + status;
  r.badge.textContent = GLYPH[status] || "·";
  r.statusLabel.className = "statusLabel " + status;
  r.statusLabel.textContent = LABEL[status] || status;
  // Track presence from the settled states, so liveness knows what's on the
  // machine. (Running states are transient — leave presence as it was.)
  M.setStatusData(model, i, status);
  refreshLiveness();
  if (RUNNING.has(status) || status === "fail") r.details.open = true; // show activity / failures
  if (status === "ok" || status === "absent") r.details.open = false; // fold completed (frame stays)
  // A finished/failed row has nothing more to add to its version delta;
  // a fresh install/remove clears any stale one. (upgrade keeps it — set just before.)
  if (r.delta && status !== "upgrading") r.delta.textContent = "";
  // Bundle-level: open while working, count active packages, auto-close
  // when the bundle's last package finishes (unless something failed).
  const be = bundleEls[model.pkgs.get(i)?.bundle];
  if (be) {
    if (RUNNING.has(status)) {
      be.active++;
      be.details.open = true;
    }
    if (status === "fail") {
      be.failed = true;
      be.details.open = true;
    }
    if (status === "ok" || status === "absent" || status === "fail") {
      be.active = Math.max(0, be.active - 1);
    }
    be.status.textContent = status === "installing"
      ? "installing…"
      : status === "uninstalling"
      ? "removing…"
      : status === "upgrading"
      ? "updating…"
      : be.failed
      ? "failed"
      : "";
    be.status.className = "bstatus " + (be.failed ? "fail" : status);
    if (be.active === 0 && !be.failed) { // all done, all good → fold to keep the page short
      be.details.open = false;
      be.status.textContent = "";
    }
  }
}

const ws = new WebSocket(`ws://${location.host}`);

// Apply = enact your decisions (on/off) against the machine; auto is left
// alone. Server acts only on the difference. No scope → all decided packages.
document.getElementById("install-all").onclick = () => applyScoped(undefined);

// Reset = drop every user toggle back to the author's defaults (clear deviations).
function resetAll() {
  M.clearAllDecisions(model);
  for (const i of model.pkgs.keys()) paintPkg(i);
  for (const name of Object.keys(bundleEls)) paintBundle(name);
  refreshLiveness();
  persistSelection();
}
// Reset is destructive to the user's choices (not to the machine), so confirm
// first — a modal mirroring the consent dialog. The button only OPENS it; the
// actual reset runs on explicit confirmation.
const resetConfirmEl = document.getElementById("reset-confirm");
document.getElementById("reset-all").onclick = () =>
  resetConfirmEl.classList.add("show");
document.getElementById("reset-confirm-no").onclick = () =>
  resetConfirmEl.classList.remove("show");
document.getElementById("reset-confirm-yes").onclick = () => {
  resetConfirmEl.classList.remove("show");
  resetAll();
};

// --- tabs ---
document.querySelectorAll(".tab").forEach((tab) => {
  tab.onclick = () => {
    document.querySelectorAll(".tab").forEach((t) =>
      t.classList.toggle("active", t === tab)
    );
    const v = tab.dataset.view;
    document.getElementById("view-setup").classList.toggle(
      "active",
      v === "setup",
    );
    document.getElementById("view-log").classList.toggle("active", v === "log");
    if (v === "log") ws.send(JSON.stringify({ type: "get-log" }));
  };
});

// --- advanced mode: a pure UI preference (per-package toggles, postures, per-
// bundle Apply). Default OFF → simple: one on/off switch per bundle. Stored in
// localStorage (browser-side, no server needed) so it sticks across sessions.
const advToggle = document.getElementById("advanced-toggle");
function applyAdvanced(on) {
  document.body.classList.toggle("advanced", on);
  advToggle.checked = on;
}
advToggle.onchange = (e) => {
  const on = e.target.checked;
  try {
    localStorage.setItem("talos.advanced", on ? "1" : "0");
  } catch {}
  applyAdvanced(on);
};
try {
  applyAdvanced(localStorage.getItem("talos.advanced") === "1");
} catch {
  applyAdvanced(false);
}

// --- consent dialog + settings ---
const consentEl = document.getElementById("consent");
function setConsent(share) {
  ws.send(JSON.stringify({ type: "set-consent", share }));
  consentEl.classList.remove("show");
}
document.getElementById("consent-yes").onclick = () => setConsent(true);
document.getElementById("consent-no").onclick = () => setConsent(false);
document.getElementById("consent-toggle").onchange = (e) =>
  ws.send(JSON.stringify({ type: "set-consent", share: e.target.checked }));
document.getElementById("clear-log").onclick = () =>
  ws.send(JSON.stringify({ type: "clear-log" }));

function renderLog(consent, history) {
  document.getElementById("consent-toggle").checked =
    !!(consent && consent.share);
  const h = document.getElementById("history");
  if (!history || !history.length) {
    h.innerHTML = '<div class="hist-empty">No install history yet.</div>';
    return;
  }
  h.innerHTML = "";
  for (const e of history.slice().reverse()) { // newest first
    const row = document.createElement("div");
    row.className = "hist-row";
    const when = (e.at || "").replace("T", " ").slice(0, 16);
    row.innerHTML = `<span class="h-when">${when}</span>` +
      `<span class="h-pkg">${e.package || ""}${
        e.version ? ` <span class="h-ver">${e.version}</span>` : ""
      }</span>` +
      `<span class="h-act ${e.ok ? "ok" : "fail"}">${e.action}${
        e.ok ? "" : " ✗"
      }</span>`;
    h.append(row);
  }
}

ws.onopen = () => overall.textContent = "checking…";
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  switch (msg.type) {
    case "plan":
      render(msg.bundles || [], msg.steps);
      applySavedSelection(msg.selection); // restore persisted decisions (yellow)
      if (msg.consent && !msg.consent.decided) consentEl.classList.add("show"); // first boot
      break;
    case "log":
      renderLog(msg.consent, msg.history);
      break;
    case "state": // ground truth from the machine
      // Detection feeds PRESENCE only (the badge/label), never the decision —
      // your yellow choices are yours. Blue auto rows just reflect the machine.
      // present: true → present, false → absent, null → indeterminate (no route
      // to constate here — don't claim absent).
      if (rows[msg.i]) {
        setStatus(
          msg.i,
          msg.present === true
            ? "ok"
            : msg.present === false
            ? "waiting"
            : "unknown",
        );
      }
      break;
    case "state-done":
      // The REAL wait is over: the machine has been probed, pills are painted.
      // THIS is what the splash now covers (not the ~12ms pty load) — so the
      // "checking what's already on board…" quip finally tells the truth.
      overall.textContent = "ready";
      hideSplash();
      break;
    case "ready":
      // Engine (pty) up — needed to RUN a command. No longer hides the splash:
      // detection (state-done) is the meaningful gate now. Kept as status only.
      if (overall.textContent === "starting…") overall.textContent = "ready";
      break;
    case "starting":
      overall.textContent = "starting…";
      break; // clicked before engine ready
    case "outdated": { // a present package has a newer version
      const r = rows[msg.i];
      if (!r) break;
      M.setOutdated(model, msg.i, true);
      if (r.delta) r.delta.textContent = `${msg.current} → ${msg.available}`;
      refreshLiveness(); // now this row's Apply is useful
      break;
    }
    case "step":
      setStatus(msg.i, msg.status);
      break;
    case "detail": // version delta for an upgrade, e.g. "2.54→2.55"
      if (rows[msg.i] && rows[msg.i].delta) {
        rows[msg.i].delta.textContent = (msg.detail || "").replace("→", " → ");
      }
      break;
    case "out": {
      const text = dec.decode(b64dec(msg.data));
      ensureTerm(msg.i).write(text);
      M.appendLog(model, msg.i, text); // keep raw for the copy button
      break;
    }
    case "apply-plan":
      // The server's full plan, in execution order. Narrow the screen to exactly
      // these steps — all of them stay visible throughout, so the whole to-do
      // list shows and progress follows down it.
      enterFocusMode((msg.plan || []).map((p) => p.i));
      break;
    case "done":
      overall.textContent = msg.nothing ? "nothing to do" : "done";
      applyRunning = false;
      exitFocusMode(); // everything reappears — failures already open, stand out
      refreshLiveness(); // unlock; re-light what's still useful
      break;
    case "overlay":
      document.getElementById("overlay-title").textContent = msg.title || "";
      document.getElementById("overlay-body").textContent = msg.body || "";
      document.getElementById("overlay").classList.add("show");
      break;
  }
};
ws.onclose = () => overall.textContent = "disconnected";
