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
// The splash lives STRICTLY for the duration of the scan: it appears when the
// analysis starts and is dismissed the instant `state-done` fires (hideSplash).
// No floor, no padding — the scan is often near-instant, so the splash may only
// flash. That honesty is the point (splash work · step 1). The 12s timer is
// a pure safety net (never fires if the scan answers first). The fade-out (~450ms)
// is the disappearance itself, not added wait.
const splash = document.getElementById("splash");
const quipEl = document.getElementById("splash-quip");
let splashDone = false;
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
function hideSplash() {
  if (splashDone) return;
  splashDone = true;
  clearInterval(quipTimer);
  splash.classList.add("hide");
  setTimeout(() => splash.remove(), 450);
}
setTimeout(hideSplash, 12000); // safety net only — state-done normally hides it first

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
  checking: "⠹",
  waiting: "·",
  installing: "⠹",
  uninstalling: "⠹",
  upgrading: "⠹",
  ok: "✓",
  absent: "·",
  self: "·",
  fail: "✗",
  forbidden: "⚠",
  unknown: "?",
};
const LABEL = {
  // Pre-scan: we haven't constated this row yet. A distinct state from "waiting"
  // (settled-absent) so we never flash "absent" before the probe answers — the
  // whole panel showing "absent" then correcting looked broken/alarming.
  checking: "checking…",
  waiting: "absent",
  installing: "installing…",
  uninstalling: "removing…",
  upgrading: "updating…",
  ok: "present",
  absent: "removed",
  self: "self-managed",
  fail: "failed",
  forbidden: "blocked by firewall",
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
  repaintAll();
}
// Repaint EVERYTHING — used after a change that can move many rows at once (a
// profile applied/removed, a restored selection). Package switches, bundle pills,
// profile chips, then the global liveness/plan preview.
function repaintAll() {
  for (const i of model.pkgs.keys()) paintPkg(i);
  for (const name of Object.keys(bundleEls)) paintBundle(name);
  paintProfiles();
  refreshLiveness();
}

// --- profiles: a bar of one-click additive selections -----------------------
const profileEls = {}; // name → card element
function renderProfiles(profiles, columns = 2) {
  const bar = document.getElementById("profiles");
  bar.innerHTML = "";
  for (const k of Object.keys(profileEls)) delete profileEls[k];
  if (!profiles.length) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  bar.style.setProperty("--cols", String(columns));
  for (const p of profiles) {
    const card = document.createElement("button");
    card.className = "profile-card";
    const hl = (p.highlights && p.highlights.length)
      ? p.highlights.join(" · ")
      : (p.packages ?? []).slice(0, 3).join(" · ");
    card.innerHTML =
      `<span class="pc-head"><span class="pc-emoji">${p.emoji || "🎯"}</span>` +
      `<span class="pc-title">${p.name}</span>` +
      `<span class="pc-count"></span></span>` +
      `<span class="pc-usage">${p.usage || p.description || ""}</span>` +
      `<span class="pc-highlights">${hl}</span>`;
    card.onclick = () => applyProfileClick(p.name);
    bar.append(card);
    profileEls[p.name] = card;
  }
  paintProfiles();
}
// Paint each card: state class (off/full/hollow = INTENT) + present-count
// (MACHINE TRUTH). Cards are inert while scanning or a run is on.
function paintProfiles() {
  const busy = applyRunning || scanning;
  for (const [name, card] of Object.entries(profileEls)) {
    const st = M.profileStateOf(model, name); // "off" | "full" | "hollow"
    card.classList.toggle("full", st === "full");
    card.classList.toggle("hollow", st === "hollow");
    card.disabled = busy;
    const { present, total } = M.profileProgress(model, name);
    const countEl = card.querySelector(".pc-count");
    if (countEl) countEl.textContent = total ? `${present}/${total}` : "";
  }
}
// Click rule (additive, per the model): clicking a profile ALWAYS applies it —
// it pulls its packages in and fills the card. It never "turns off" a profile;
// you lose a profile only by DESELECTING one of its packages (that turns the
// card hollow). This matches the user's mental model: a profile is a preset you
// apply, not a light you toggle. (Reset clears all active profiles at once.)
function applyProfileClick(name) {
  M.applyProfile(model, name);
  repaintAll();
  persistSelection();
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
  // Keep self-managed labelling live when the user toggles a config-atom off:
  // paintPkg/refreshLiveness (the toggle path) never touch statusLabel — only
  // setStatus does, on scan/step messages — so without this the label would
  // stay stale ("present") until the next scan. Toggling back IN restores
  // normal behaviour on the next scan/refresh (setStatus repaints it).
  const p = model.pkgs.get(i);
  if (p?.isConfig && M.desiredOf(model, i) === "absent") {
    r.statusLabel.textContent = "self-managed";
    r.statusLabel.className = "statusLabel self";
    r.badge.textContent = "·";
    r.badge.className = "badge self";
  }
}
// Render the row's version display — the SINGLE place numbers appear (the delta
// slot). Reads versionSummary so it can never repeat a number or show a third:
//   { from, to, pinned }  (see model.versionSummary)
//     no arrow (to === null) → one number; 📌 before it when pinned:"from"
//     arrow (to set)          → from → to; 📌 before `to` when pinned:"to"
// The 📌 marks the pin exactly once and reads NEUTRAL (dim), not green — being at
// or moving to your pin isn't something to push; the pin is a fact, not a nudge.
// A pinned "to" migration still shows plainly, but the button's own colour (from
// isActionable) decides vivid-vs-neutral, not this text.
// ONE colour language, shared with the buttons (refreshLiveness):
//   current version   → BLUE  (.v-cur)    — the machine state, what a button acts FROM
//   pushed target      → GREEN (.v-push)   — install/upgrade Apply WILL run
//   removal target     → RED   (.v-remove) — uninstall → "absent"
//   not-pushed target  → GREY  (.v-muted)  — downgrade / upgrade past a pin (manual only)
// So the eye follows one thread: a green button leads to a green target, a red
// button to a red "absent", a blue/avail button to a grey (not-pushed) number.
// The 📌 pin marker rides along inside its number's colour (no separate tint).
function paintVersion(i) {
  const r = rows[i];
  if (!r || !r.delta) return;
  const v = M.versionSummary(model, i);
  const cur = (t) => `<span class="v-cur">${t}</span>`;
  const push = (t) => `<span class="v-push">${t}</span>`;
  const remove = (t) => `<span class="v-remove">${t}</span>`;
  const muted = (t) =>
    `<span class="v-muted" title="available — not pushed (manual)">${t}</span>`;
  const pin = (t) => `📌${t}`;

  // A PENDING uninstall reads current(blue) → absent(red) — same red as its
  // button. Keyed on the PLAN action (actionOf), not buttonAction: the latter is
  // the manual invert (still "upgrade" for an outdated pkg), while actionOf is
  // what Apply will really do — "uninstall" only when the user wants it absent.
  if (M.actionOf(model, i) === "uninstall" && v.from) {
    r.delta.innerHTML = `${cur(v.from)} → ${remove("absent")}`;
    return;
  }
  if (!v.from) {
    r.delta.innerHTML = ""; // absent / no version to show
    return;
  }
  if (v.to === null) {
    // One number — the current state, blue. 📌 when it IS the pin (at rest).
    r.delta.innerHTML = cur(v.pinned === "from" ? pin(v.from) : v.from);
    return;
  }
  // Two numbers: current(blue) → target. Target colour = the button that leads
  // there: muted (downgrade / upgrade-past-pin) → grey; else a real push → green.
  const left = cur(v.pinned === "from" ? pin(v.from) : v.from);
  const rightTxt = v.pinned === "to" ? pin(v.to) : v.to;
  const right = v.muted ? muted(rightTxt) : push(rightTxt);
  r.delta.innerHTML = `${left} → ${right}`;
}
function setToggle(i, state, opts = {}) {
  if (!M.setDecision(model, i, state)) return; // locked → refused
  paintPkg(i);
  const b = model.pkgs.get(i)?.bundle; // repaint the parent bundle pill:
  if (b) paintBundle(b); // its in/out/MIXED may have changed
  paintProfiles(); // a manual toggle can make a profile go hollow (or full again)
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
// True from render until `state-done`: the machine scan is still running, so the
// plan is INCOMPLETE — acting now would treat un-probed rows as absent and act on
// a reality the user never saw. Freeze every action (general, bundle, row) until
// the scan settles, exactly like applyRunning freezes them during a run.
let scanning = false;
function refreshLiveness() {
  const busy = applyRunning || scanning; // no action while running OR still scanning
  const ids = Object.keys(rows).map(Number);
  // Per-row buttons: light the ones that would act; all stay clickable at
  // hover for a manual re-run, but disabled while a run is on.
  for (const i of ids) {
    const r = rows[i];
    const act = buttonAction(i); // always the invert action (label)
    const inPlan = isActionable(i); // would a plain Apply act here?
    const show = !!act && !busy;
    // Colour follows the PLAN: directional green/red only if Apply will act
    // now; otherwise blue (available manual escape hatch, no pending change).
    r.apply.classList.toggle("live", show && inPlan);
    r.apply.classList.toggle("add", show && inPlan && act.dir === "add");
    r.apply.classList.toggle("remove", show && inPlan && act.dir === "remove");
    r.apply.classList.toggle("avail", show && !inPlan);
    r.apply.textContent = act ? act.verb : "—"; // button IS the action; — if none possible
    r.apply.disabled = busy || !act;
    // Tint the WHOLE row when it's in the plan — the change is unmissable, not
    // hidden in a small button. When in-plan, act.dir is the plan's direction.
    r.details.classList.toggle("plan-add", inPlan && act && act.dir === "add");
    r.details.classList.toggle(
      "plan-remove",
      inPlan && act && act.dir === "remove",
    );
    paintPkg(i); // switch colour follows the plan — refresh it as machine state lands
    paintVersion(i); // version display follows the plan too: toggling a row OUT
    // flips its target to "→ absent" (uninstall), so it must repaint here, not
    // only on state/outdated messages. Without this the number stayed stale
    // (e.g. "→ 15.2.0 update") while the button already said uninstall.
  }
  // Bundle buttons: live if any of their packages would act.
  for (const name of Object.keys(bundleEls)) {
    const be = bundleEls[name];
    const live = M.bundleAnyActionable(model, name);
    be.apply.classList.toggle("live", live && !busy);
    be.apply.disabled = busy;
    paintBundle(name); // bundle switch colour follows the plan too
    // Preview the plan AT REST: a bundle with pending actions opens, a stable
    // one folds. Only when idle — during a run the execution logic (setStatus)
    // owns open/close, and we never fight the user's manual toggle mid-run.
    if (!busy) be.details.open = live;
  }
  // Global button: live if anything anywhere would act.
  const g = document.getElementById("install-all");
  g.classList.toggle("live", ids.some(isActionable) && !busy);
  g.disabled = busy;
  // Reset is available only when it would do something (model.canReset decides:
  // a moved package or an active profile). The view just reflects that verdict.
  const reset = document.getElementById("reset-all");
  if (reset) reset.disabled = busy || !M.canReset(model);
  // Refresh is available whenever we're idle — it only re-reads the machine.
  const refresh = document.getElementById("refresh-all");
  if (refresh) refresh.disabled = busy;
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
  // The server re-scans presence before it replies with the plan — a silent gap.
  // Dim #steps so the re-scan is VISIBLE and interaction is locked. Cleared on
  // `apply-plan` (focus mode takes over) or `done` (nothing to do, no plan).
  stepsEl.classList.add("steps-refreshing");
  // Focus mode engages on the server's `apply-plan` reply (it computes the plan),
  // not here — so we show the exact set of steps that will run.
  ws.send(JSON.stringify({ type: "apply", on, off, scope: scopeIdx }));
}

function render(bundles, steps, profiles = [], columns = 2) {
  M.loadPlan(model, bundles, steps, profiles);
  // The scan starts now and won't settle until `state-done`. Freeze actions and
  // dim the panel until then: rows show "checking…", nothing is clickable, and
  // each lights up as its probe answers — no acting on an incomplete plan.
  scanning = true;
  stepsEl.classList.add("scanning");
  renderProfiles(profiles, columns);
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
    // When the row is first opened, flush any pending detection evidence into its
    // terminal. We DON'T write it at scan time: that would force-create a terminal
    // per row (17 at once) while the row is collapsed (host height 0 → xterm renders
    // nothing). Deferring to open means the host is visible and xterm paints correctly.
    d.addEventListener("toggle", () => {
      if (d.open) flushProbe(s.i);
    });
    const sum = document.createElement("summary");
    const chk = makeToggle();
    chk.onclick = (e) => {
      e.preventDefault();
      e.stopPropagation();
      flipPkg(s.i);
    };
    const badge = document.createElement("span");
    badge.className = "badge checking";
    badge.textContent = "⠹"; // pre-scan spinner, not a verdict yet
    const name = document.createElement("span");
    name.className = "name";
    const inProfiles = M.profilesForPkg(model, s.i);
    const profTags = inProfiles.length
      ? ` <span class="pkg-profiles" title="In profiles: ${
        inProfiles.map((p) => p.name).join(", ")
      }">${inProfiles.map((p) => p.emoji).join("")}</span>`
      : "";
    name.innerHTML = `${s.name}` + profTags +
      (s.description ? ` <span class="desc">— ${s.description}</span>` : "");
    const delta = document.createElement("span"); // version delta on upgrade, e.g. 2.54 → 2.55
    delta.className = "verdelta";
    const st = document.createElement("span");
    st.className = "statusLabel checking";
    st.textContent = "checking…"; // until its probe answers — never pre-say "absent"
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
      overall.textContent = act.type === "diff" ? "diffing…" : "applying…";
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
      copyText(text).then((ok) => {
        copy.innerHTML = ok ? ICON_OK : ICON_COPY;
        if (ok) setTimeout(() => copy.innerHTML = ICON_COPY, 1200);
      });
    };
    const host = document.createElement("div");
    host.className = "term-host";
    // Firewall-403 recovery banner — the DURABLE surface (the modal is transient).
    // Hidden until a `forbidden` event; then it carries the blocked-URL link plus
    // Retry / Give-up right beside this step's terminal output (the evidence).
    const fbBanner = document.createElement("div");
    fbBanner.className = "fb-banner";
    panel.append(fbBanner, copy, host);
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
      fbBanner,
      term: null,
      probeText: null, // detection evidence, stashed at scan, flushed on first open
      probeWritten: false,
    };
    paintPkg(s.i); // initial pill: locked word for mandatory/forbidden, else auto
  }
  // Now that every bundle knows its packages, paint bundle pills (locked or auto).
  for (const name of Object.keys(bundleEls)) refreshBundleChk(name);
  // Freeze all actions immediately: the scan (scanning=true) hasn't settled, so
  // no button should be live until state-done proves the plan complete.
  refreshLiveness();
}

function ensureTerm(i) {
  const r = rows[i];
  if (!r.term) {
    r.term = newTerm();
    r.term.open(r.host);
  }
  return r.term;
}

// Copy text to the clipboard, robustly. navigator.clipboard is often UNAVAILABLE in
// a WKWebView served over plain http://127.0.0.1 (non-secure context) — it either
// doesn't exist or its promise rejects, and the old code had no fallback, so copying
// failed silently. Try the modern API, then fall back to a hidden-textarea +
// execCommand("copy") (works in non-secure contexts). Returns true on success.
async function copyText(text) {
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch (_) {
    // fall through to the legacy path
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch (_) {
    return false;
  }
}

// Detection evidence: the exact command run at scan, its raw output, and the verdict.
// The "don't trust me, check it yourself" surface — when a result looks wrong, the
// operator sees precisely which command produced it. English only.
//
// We STASH it on the row at scan time and only WRITE it when the row is first opened
// (flushProbe, via the details "toggle" handler). Writing at scan would force-create
// 17 terminals in collapsed rows (host height 0), where xterm paints nothing.
function stashProbe(i, probe, state) {
  if (!rows[i]) return;
  const verdict = state.present === true
    ? `present${state.version ? ` (version ${state.version})` : ""}`
    : state.present === false
    ? "absent"
    : "indeterminate";
  const out = (probe.output || "").replace(/\r?\n/g, "\r\n").replace(/\r\n$/, "");
  // Cyan bold prompt line for the command, raw output verbatim, dim verdict footer.
  rows[i].probeText = `\x1b[36;1m$ ${probe.cmdline}\x1b[0m\r\n` +
    (out ? out + "\r\n" : "") +
    `\x1b[2m→ ${verdict} · exit ${probe.code}\x1b[0m\r\n`;
  rows[i].probeWritten = false;
  // Also feed the copy buffer: the detection evidence must be copyable too, not just
  // install output. Without this, copying a scan-only row yields an empty string.
  M.appendLog(model, i, rows[i].probeText.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  // If the row is already open (rare: a re-scan while expanded), flush now.
  if (rows[i].details?.open) flushProbe(i);
}

// Write the stashed evidence into the row's terminal, once, when it's visible.
function flushProbe(i) {
  const r = rows[i];
  if (!r || !r.probeText || r.probeWritten) return;
  ensureTerm(i).write(r.probeText);
  r.probeWritten = true;
}

const RUNNING = new Set(["installing", "uninstalling", "upgrading"]);

// Braille spinner: the static "⠹" looked frozen. A single global ticker cycles
// the canonical braille frames on every badge currently in an active state
// (checking / installing / uninstalling / upgrading), so working rows visibly
// animate. One interval for the whole panel — cheap, and only touches spinning
// badges. The badge's status CLASS (set by setStatus) is the source of truth.
const SPIN_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPIN_STATES = ["checking", "installing", "uninstalling", "upgrading"];
let spinFrame = 0;
setInterval(() => {
  spinFrame = (spinFrame + 1) % SPIN_FRAMES.length;
  const f = SPIN_FRAMES[spinFrame];
  for (const r of Object.values(rows)) {
    if (SPIN_STATES.some((s) => r.badge.classList.contains(s))) {
      r.badge.textContent = f;
    }
  }
}, 90);

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

// Waiting-window banner: shown while an installer waits behind the panel, cleared
// when the step ends (or the socket drops). One live banner; latest message wins.
const waitbar = document.getElementById("waitbar");
function showWait(text) {
  waitbar.textContent = text;
  waitbar.hidden = false;
}
function hideWait() {
  waitbar.hidden = true;
}

// Firewall-403 recovery UI. Two surfaces, one state: the ROW banner is durable
// (the source of truth — survives dismissing the modal, one per row), the MODAL
// is a transient attention-grabber for the currently-blocked step. `fbActive`
// holds the step index the modal currently speaks for, so its Retry/Give-up act
// on the right row. url may be null (nothing extractable) → link-less message.
const fbEl = document.getElementById("forbidden");
const fbLinkWrap = document.querySelector(".fb-link-wrap");
const fbLink = document.getElementById("fb-link");
const fbMoreInfo = document.getElementById("fb-moreinfo");
const fbStepOpen = document.getElementById("fb-open");
const fbStepRetry = document.getElementById("fb-retry");
let fbActive = null;
let fbActiveUrl = null; // url the modal's "Open blocked page" step acts on

// The stepper's amber highlight rides the CURRENT step, so the primary action is
// never stale. "open" → step 1 lit; "retry" → step 1 done (green ✓), step 3 lit.
// The passive step 2 never lights (nothing to click in Talos).
function setFbStep(current) {
  const done = current === "retry"; // step 1 is done once we've opened the page
  fbStepOpen.classList.toggle("active", current === "open");
  fbStepOpen.classList.toggle("done", done);
  fbStepOpen.querySelector(".fb-marker").textContent = done ? "✓" : "1";
  fbStepRetry.classList.toggle("active", current === "retry");
}

// Ask the server to open the blocked page in a browser beside the panel. On
// demand (a step click), never auto — so the user reads the guidance before the
// window covers it. Falls back to the clickable link if the server can't open.
function openForbidden(url) {
  if (url) ws.send(JSON.stringify({ type: "open-forbidden", url }));
}

function renderLink(el, url) {
  el.href = url || "#";
  el.textContent = url || "";
}

function showForbidden(i, url) {
  const r = rows[i];
  if (!r) return;
  // Row banner (durable) — guidance + Retry/Give-up, built once. No raw link by
  // default (it's under "More info…" in the modal); the guidance says approve
  // access, don't download — Talos re-runs the download on Retry.
  r.fbBanner.innerHTML =
    `<b>⚠ A download was blocked by the corporate firewall (403).</b><br>` +
    `Open the blocked page and approve the access request — you don't need to ` +
    `download anything. Then Retry and Talos fetches it for you.` +
    `<div class="fb-banner-actions">` +
    (url ? `<button data-fb="open">Open blocked page</button>` : "") +
    `<button data-fb="retry">Retry</button>` +
    `<button data-fb="giveup">Give up</button></div>`;
  r.fbBanner.classList.add("show");
  r.details.open = true;
  const openBtn = r.fbBanner.querySelector('[data-fb="open"]');
  if (openBtn) openBtn.onclick = () => openForbidden(url);
  r.fbBanner.querySelector('[data-fb="retry"]').onclick = () => retryStep(i);
  r.fbBanner.querySelector('[data-fb="giveup"]').onclick = () => giveUp(i);
  // Transient modal — the guided stepper for the active block.
  fbActive = i;
  fbActiveUrl = url;
  renderLink(fbLink, url);
  // With a URL: step 1 (Open) is the live action. Without one (rare — nothing
  // extractable): step 1 can't act, so disable it and start the highlight on
  // Retry (the user clears access however they can, then retries).
  fbStepOpen.disabled = !url;
  setFbStep(url ? "open" : "retry");
  // Reset the "More info…" disclosure each time: link hidden, prompt shown (only
  // if there's a URL to reveal).
  fbLinkWrap.hidden = true;
  fbMoreInfo.classList.toggle("empty", !url);
  fbEl.classList.add("show");
}

function clearForbidden(i) {
  const r = rows[i];
  if (r && r.fbBanner) {
    r.fbBanner.classList.remove("show");
    r.fbBanner.innerHTML = "";
  }
  if (fbActive === i) {
    fbEl.classList.remove("show");
    fbActive = null;
    fbActiveUrl = null;
  }
}

function retryStep(i) {
  if (applyRunning) return;
  const act = buttonAction(i); // recomputed from current model → the original action
  if (!act) return;
  clearForbidden(i);
  applyRunning = true;
  refreshLiveness();
  overall.textContent = "retrying…";
  ws.send(JSON.stringify({ type: "retry-step", i, action: act.type }));
}

function giveUp(i) {
  // Settle the row as a plain failure locally — the user chose to stop. Nothing
  // to send: the step already exited; this just drops the recovery UI.
  clearForbidden(i);
  setStatus(i, "fail");
}

// "More info…" reveals the raw blocked URL and hides its own prompt — for the
// rare user who wants to see or copy the exact address.
document.getElementById("fb-moreinfo").onclick = () => {
  fbLinkWrap.hidden = false;
  fbMoreInfo.classList.add("empty");
};

// Step cards act on the block the modal speaks for. Clicking Open opens the page
// AND advances the highlight to Retry — so when the user comes back, Retry (not
// Open) is the lit primary action.
fbStepOpen.onclick = () => {
  if (fbActive === null) return;
  openForbidden(fbActiveUrl);
  setFbStep("retry");
};
fbStepRetry.onclick = () => {
  if (fbActive !== null) retryStep(fbActive);
};
document.getElementById("fb-giveup").onclick = () => {
  if (fbActive !== null) giveUp(fbActive);
};

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
  // "present" is NO LONGER a word: presence is carried by colour — the ✓ badge and
  // the yellow installed version (paintVersion). So `ok` clears the label; every
  // other state keeps its word (absent / checking… / failed / —). A present pkg
  // with no version (plugin/config-atom) still reads present via the ✓ badge.
  r.statusLabel.textContent = status === "ok" ? "" : (LABEL[status] || status);
  // Track presence from the settled states, so liveness knows what's on the
  // machine. (Running states are transient — leave presence as it was.)
  M.setStatusData(model, i, status);
  refreshLiveness();
  if (RUNNING.has(status) || status === "fail" || status === "forbidden") {
    r.details.open = true; // show activity / failures / firewall recovery
  }
  if (status === "ok" || status === "absent") r.details.open = false; // fold completed (frame stays)
  // Version display is owned by paintVersion (reads versionSummary). While a row
  // is RUNNING, blank the delta (transient); once settled, repaint from the model
  // so the pin marker / cur→avail reappear correctly. Never write numbers here.
  if (r.delta) {
    if (RUNNING.has(status)) r.delta.innerHTML = "";
    else paintVersion(i);
  }
  // Bundle-level: open while working; STAY open between packages. Packages run
  // SERIALLY (one manager at a time), so `active` dips to 0 between each — folding
  // on active===0 mid-run made the card flap shut/open per package. Folding of a
  // finished bundle now happens ONCE, at the end of the whole Apply (see `done`).
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
  }
}

const ws = new WebSocket(`ws://${location.host}`);

// Apply = enact your decisions (on/off) against the machine; auto is left
// alone. Server acts only on the difference. No scope → all decided packages.
document.getElementById("install-all").onclick = () => applyScoped(undefined);

// Reset = drop every user toggle AND active profile back to the author's
// defaults (clearAllDecisions clears both). repaintAll refreshes chips too.
function resetAll() {
  M.clearAllDecisions(model);
  repaintAll();
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

// Refresh: re-scan the machine on demand. Freezes + dims the panel like the
// initial scan (scanning=true), sets every row back to "checking…", and asks the
// server to re-detect. Touches ONLY detection — the user's selection is untouched
// (unlike Reset). state/state-done repaint and unfreeze as the probes answer.
document.getElementById("refresh-all").onclick = () => {
  if (applyRunning || scanning) return; // don't stack a scan on a run or a scan
  scanning = true;
  stepsEl.classList.add("scanning");
  for (const i of Object.keys(rows).map(Number)) setStatus(i, "checking");
  refreshLiveness();
  paintProfiles();
  overall.textContent = "checking…";
  ws.send(JSON.stringify({ type: "rescan" }));
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

// --- sudo password dialog (masked; local WS only, never stored) ---
const sudoEl = document.getElementById("sudo");
const sudoInput = document.getElementById("sudo-input");
function showSudo() {
  sudoInput.value = "";
  sudoEl.classList.add("show");
  sudoInput.focus();
}
function hideSudo() {
  sudoEl.classList.remove("show");
  sudoInput.value = ""; // never keep the secret in the DOM after use
}
document.getElementById("sudo-form").onsubmit = (e) => {
  e.preventDefault();
  ws.send(JSON.stringify({ type: "sudo-pw", pw: sudoInput.value }));
  hideSudo();
};
document.getElementById("sudo-cancel").onclick = () => {
  ws.send(JSON.stringify({ type: "sudo-cancel" }));
  hideSudo();
};

// Stamp the UI with the exact source snapshot this exe was built from — in the
// window title (always visible) and the Log & settings footer. This is the
// answer to "which binary is actually running?" that cost us a long detour.
function showBuild(build) {
  // Order mirrors `jj log`: change id leads, commit id (git-sha) trails.
  const label = `${build.change} · ${build.sha}`;
  document.title = `Talos · ${build.change}`;
  const el = document.getElementById("buildinfo");
  if (el) {
    el.textContent = `build ${label} — ${build.builtAt}`;
  }
}

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
      render(
        msg.bundles || [],
        msg.steps,
        msg.profiles || [],
        msg.profileColumns,
      );
      applySavedSelection(msg.selection); // restore persisted decisions (yellow)
      if (msg.consent && !msg.consent.decided) consentEl.classList.add("show"); // first boot
      if (msg.build) showBuild(msg.build); // stamp the UI with the exact source snapshot
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
        // Record the live installed version BEFORE setStatus, so the pin
        // comparison in actionOf/buttonAction sees it when liveness refreshes.
        // "" when the probe found no version (absent, or a route with no version).
        M.setInstalledVersion(model, msg.i, msg.version || "");
        // A config-atom the user turned OFF is self-managed: neutral, not
        // "absent". Talos won't touch the file, so we don't judge it — the row
        // says "self-managed" and its button offers a diff (see buttonAction).
        if (model.pkgs.get(msg.i)?.isConfig && M.desiredOf(model, msg.i) === "absent") {
          setStatus(msg.i, "self");
          break;
        }
        setStatus(
          msg.i,
          msg.present === true
            ? "ok"
            : msg.present === false
            ? "waiting"
            : "unknown",
        );
        // Indeterminate WITH a reason (e.g. a missing prerequisite) → show why,
        // in place of the bare "—", so the row isn't a silent mystery.
        if (msg.present === null && msg.reason) {
          rows[msg.i].statusLabel.textContent = msg.reason;
        }
        // Version numbers live in ONE place now (paintVersion → the delta slot),
        // driven by versionSummary. The statusLabel keeps the PRESENCE WORD
        // ("present") so we never show the same number twice. paintVersion runs
        // after the outdated event too (it reads the model), so cur→avail and the
        // pin marker land in the same single element.
        paintVersion(msg.i);
        // Installed OUTSIDE winget → append the provenance flag (winget can't
        // upgrade/uninstall it). Present stays present; this just says HOW.
        if (msg.present === true && msg.external) {
          rows[msg.i].statusLabel.textContent += " · external";
          rows[msg.i].statusLabel.classList.add("external");
        }
        // Stash the DETECTION EVIDENCE (command + output + verdict). Written into the
        // row's terminal when it's first opened — so a doubted result can be checked
        // without trusting the badge: "which command decided this?".
        if (msg.probe && msg.probe.cmdline) stashProbe(msg.i, msg.probe, msg);
      }
      break;
    case "state-done":
      // The REAL wait is over: the machine has been probed, pills are painted.
      // THIS is what the splash now covers (not the ~12ms pty load) — so the
      // "checking what's already on board…" quip finally tells the truth.
      // Stamp WHEN the scan completed — the model stays clock-free, so the view
      // sets this. Foundation for a future "reuse if fresh (<TTL)" optimisation.
      model.detectedAt = Date.now();
      // Scan settled: the plan is now complete and trustworthy. Unfreeze actions
      // and un-dim — the general/bundle/row buttons come alive via refreshLiveness.
      scanning = false;
      stepsEl.classList.remove("scanning");
      refreshLiveness();
      paintProfiles();
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
      // Carry the available version into the model, then let paintVersion render
      // it (a pinned package ignores this — its pin owns the display, so no third
      // number). One renderer, no repetition.
      M.setOutdated(model, msg.i, true, msg.available);
      paintVersion(msg.i);
      refreshLiveness(); // now this row's Apply is useful
      break;
    }
    case "step":
      setStatus(msg.i, msg.status);
      // A step that settled ok clears any lingering firewall recovery UI (the
      // 403 was transient — the exit code is truth). Retry / new run also reset it.
      if (msg.status === "ok" || msg.status === "absent") clearForbidden(msg.i);
      break;
    case "forbidden":
      // Live: a download was blocked by the firewall. Show the row banner (durable)
      // and the transient modal. The server already opened the blocked page beside
      // the panel; url may be null (nothing extractable → link-less message).
      showForbidden(msg.i, msg.url || null);
      break;
    case "sudo-prompt":
      // A step hit a sudo "Password:" prompt (e.g. removing a GUI app). Show the
      // masked field. The server asks ONCE per Apply (model C), then reuses it.
      showSudo();
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
      stepsEl.classList.remove("steps-refreshing"); // scan done, plan is here
      enterFocusMode((msg.plan || []).map((p) => p.i));
      break;
    case "done":
      overall.textContent = msg.nothing ? "nothing to do" : "done";
      applyRunning = false;
      stepsEl.classList.remove("steps-refreshing"); // net: scan found nothing → no apply-plan
      exitFocusMode(); // everything reappears — failures already open, stand out
      // Fold bundles that finished cleanly — ONCE, now the whole run is over (not
      // between packages). A failed bundle stays open so its error is visible.
      for (const be of Object.values(bundleEls)) {
        be.active = 0;
        if (!be.failed) {
          be.details.open = false;
          be.status.textContent = "";
        }
      }
      refreshLiveness(); // unlock; re-light what's still useful
      break;
    case "overlay":
      document.getElementById("overlay-title").textContent = msg.title || "";
      document.getElementById("overlay-body").textContent = msg.body || "";
      document.getElementById("overlay").classList.add("show");
      break;
    // An installer opened a window behind the panel while a step ran. Say so —
    // the panel isn't frozen, it's waiting. wait-window: we found the window and
    // flashed its taskbar button (always) + TRIED to raise it (msg.pushed says if
    // that worked — the foreground-lock often refuses it). State a fact + a
    // pointer, never claim a raise that may not have happened. wait-silent:
    // nothing found but the tool went quiet (likely a UAC prompt on the secure
    // desktop). wait-clear: the step ended → drop the banner.
    case "wait-window": {
      const who = msg.title ? `“${msg.title}”` : "An installer window";
      showWait(
        msg.pushed
          ? `${who} is waiting for you — we’ve brought it to the front.`
          : `${who} is waiting for you, behind this window — it’s flashing in your taskbar.`,
      );
      break;
    }
    case "wait-silent":
      showWait(
        "This is taking a while — an installer may be waiting on another screen. Look for a permission prompt.",
      );
      break;
    case "wait-clear":
      hideWait();
      break;
  }
};
ws.onclose = () => {
  overall.textContent = "disconnected";
  stepsEl.classList.remove("steps-refreshing"); // don't leave the dim stuck on
  hideWait(); // don't leave the banner stuck on
};
