// app.js — the Talos panel UI. Pure decision logic lives in decision.js
// (imported below), shared with the server so the rule can never drift.
import {
  forbiddenMessage,
  postureDefault as _postureDefault,
} from "./decision.js";
import * as M from "./model.js";
const model = M.createModel();

const stepsEl = document.getElementById("steps");
const overall = document.getElementById("overall");
const b64dec = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const dec = new TextDecoder();
const rows = {}; // i → { details, badge, statusLabel, host, term }

// --- startup splash: NARRATES the scan instead of entertaining during it ---
// The scan is SERIAL (server.rs scan_and_emit) and the server emits one `state`
// per package as its verdict lands. So "what is Talos doing right now?" has a
// true answer at every instant — we show it. This replaced a carousel of rotating
// quips, which was an admission that we could not describe the wait: on a slow
// Windows box the scan runs ~25s, and 25s of jokes with no visible progress reads
// as "it hung". Naming the package + i/n turns the same duration into a story.
// DO NOT reintroduce filler here: if a wait cannot be described, describe THAT.
//
// The splash lives STRICTLY for the duration of the scan: it appears at load and
// is dismissed when `state-done` fires (hideSplash). No floor, no padding — a fast
// scan means it only flashes, and that honesty is the point. The safety-net timer
// must outlast a realistic slow scan, else it lifts mid-scan and exposes the
// half-painted accordion (the "second wait" people reported). The fade-out (~450ms)
// is the disappearance itself, not added wait.
const SPLASH_SAFETY_MS = 120000; // never fires if state-done answers first
const splash = document.getElementById("splash");
const splashNow = document.getElementById("splash-now");
const splashCount = document.getElementById("splash-count");
const splashBar = document.getElementById("splash-bar");
let splashDone = false;
let scanTotal = 0; // package count, known once the plan is rendered
let scanSeen = 0; // verdicts received so far

// Announce the package about to be probed. `n` is the 1-based position.
function splashProgress(name, n) {
  if (splashDone) return;
  splashNow.textContent = name ? `checking ${name}…` : "checking…";
  if (scanTotal > 0) {
    splashCount.textContent = `${n} of ${scanTotal}`;
    splashBar.classList.add("determinate");
    splashBar.firstElementChild.style.width = `${(n / scanTotal) * 100}%`;
  }
}

// The SAME narration for the Apply's re-scan veil (#steps-refresh). Was a frozen
// "Plotting the gallop…" over a wait that got LONGER when the scan went serial —
// the one mute surface left. The re-scan is now scoped to the diff, so `total` is a
// handful, not 30: this reads as a short countdown rather than a wall.
const refreshNow = document.getElementById("refresh-now");
const refreshCount = document.getElementById("refresh-count");
const refreshBar = document.getElementById("refresh-bar");
function refreshProgress(name, nth, total) {
  refreshNow.textContent = name ? `checking ${name}…` : "checking…";
  if (total > 0) {
    refreshCount.textContent = `${nth} of ${total}`;
    refreshBar.classList.add("determinate");
    refreshBar.firstElementChild.style.width = `${(nth / total) * 100}%`;
  }
}
// Back to the resting frame, so the NEXT Apply doesn't open on the tail of the last
// one ("12 of 12" under a bar already full would read as instantly finished).
function resetRefreshProgress() {
  refreshNow.textContent = "Plotting the gallop…";
  refreshCount.textContent = "";
  refreshBar.classList.remove("determinate");
  refreshBar.firstElementChild.style.width = "";
}

function hideSplash() {
  if (splashDone) return;
  splashDone = true;
  splash.classList.add("hide");
  setTimeout(() => splash.remove(), 450);
}
setTimeout(hideSplash, SPLASH_SAFETY_MS); // safety net only — state-done normally hides it first

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
const ICON_COPY =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>';
const ICON_OK =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';

// --- persistence: only USER-MOVED toggles are saved (untouched = absence).
// Saved by NAME (bundle::package) so it survives bundles being added/reordered.
function persistSelection() {
  ws.send(JSON.stringify({
    type: "set-selection",
    selection: {
      pkgs: M.persistablePkgs(model),
      personal: M.persistablePersonal(model), // My setup members (spec §14)
      activeBundles: M.persistableActiveBundles(model), // which cards are on
      ui: { // display prefs — one config file for all (§ config-unique)
        advanced: document.body.classList.contains("advanced"),
        showAll: document.body.classList.contains("show-all"),
        theme: document.documentElement.dataset.theme || "",
      },
    },
  }));
}
function applySavedSelection(sel) {
  M.applySavedSelection(model, sel);
  M.applySavedPersonal(model, sel?.personal); // restore My setup members (spec §14)
  M.applySavedActiveBundles(model, sel?.activeBundles); // restore active cards + cascade
  // UI prefs live in the same file now (not localStorage): restore them here.
  applyAdvanced(!!sel?.ui?.advanced);
  applyShowAll(!!sel?.ui?.showAll);
  applyTheme(sel?.ui?.theme || "");
  repaintAll();
}
// Repaint EVERYTHING — used after a change that can move many rows at once (a
// profile applied/removed, a restored selection). Package switches, profile
// chips, then the global liveness/plan preview.
function repaintAll() {
  for (const i of model.pkgs.keys()) paintPkg(i);
  paintProfiles();
  refreshLiveness();
}

// --- profiles: a bar of one-click additive selections -----------------------
const profileEls = {}; // name → card element
function makeProfileCard(p) {
  const card = document.createElement("button");
  card.className = "profile-card";
  const hl = (p.highlights && p.highlights.length)
    ? p.highlights.join(" · ")
    : (p.packages ?? []).slice(0, 3).join(" · ");
  // Dependency link, stated textually now that the staircase indent is gone.
  const needs = (p.needs ?? []).length
    ? `<span class="pc-needs">needs ${p.needs.join(" · ")}</span>`
    : "";
  card.innerHTML =
    `<span class="pc-head"><span class="toggle pc-switch"><span class="knob"></span></span>` +
    `<span class="pc-emoji">${p.emoji || "🎯"}</span>` +
    `<span class="pc-title">${p.name}</span>` +
    `<span class="pc-count"></span></span>` +
    `<span class="pc-usage">${p.usage || p.description || ""}</span>` +
    `<span class="pc-highlights">${hl}</span>` +
    needs;
  card.onclick = () => applyProfileClick(p.name);
  return card;
}
function renderProfiles(_profiles, _columns = 2) {
  const bar = document.getElementById("profiles");
  bar.innerHTML = "";
  for (const k of Object.keys(profileEls)) delete profileEls[k];
  // Render from the MODEL (it holds the always-on "My setup" the server never sends).
  const profiles = [...model.profiles.values()];
  if (!profiles.length) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  // TWO COLUMNS (spec §24): the dependency CHAIN on the left, STANDALONE bundles
  // (incl. My setup) on the right. A bundle is "chain" if it has `needs` OR is
  // needed by another; the rest are standalone. Chain kept in declared order so
  // Base→Documents→Data→Development reads top-to-bottom.
  const neededByOthers = new Set();
  for (const p of profiles) for (const n of p.needs ?? []) neededByOthers.add(n);
  const isChain = (p) => (p.needs?.length ?? 0) > 0 || neededByOthers.has(p.name);
  // Depth in the dependency chain = how many `needs` deep (Base=0, Documents=1,
  // Data=2, Development=3). Sort the chain column by depth so the staircase reads
  // top-down in dependency order, not file-load order. Cycle-guarded.
  const byName = new Map(profiles.map((p) => [p.name, p]));
  const depthOf = (p, seen = new Set()) => {
    if (seen.has(p.name)) return 0; // cycle guard
    seen.add(p.name);
    const deps = (p.needs ?? []).map((n) => byName.get(n)).filter(Boolean);
    return deps.length ? 1 + Math.max(...deps.map((d) => depthOf(d, seen))) : 0;
  };
  const chainCol = document.createElement("div");
  chainCol.className = "bundle-col chain";
  const soloCol = document.createElement("div");
  soloCol.className = "bundle-col standalone";
  const chain = profiles.filter(isChain).sort((a, b) => depthOf(a) - depthOf(b));
  const solo = profiles.filter((p) => !isChain(p));
  // A header per column so the two natures read at a glance.
  const colHead = (text) => {
    const h = document.createElement("div");
    h.className = "bundle-col-head";
    h.textContent = text;
    return h;
  };
  // Cards live in an inner wrapper so the staircase nth-child counts only cards,
  // not the header.
  const chainCards = document.createElement("div");
  chainCards.className = "bundle-col-cards";
  const soloCards = document.createElement("div");
  soloCards.className = "bundle-col-cards";
  if (chain.length) chainCol.append(colHead("Levels — each builds on the one above"), chainCards);
  if (solo.length) soloCol.append(colHead("Add-ons — independent"), soloCards);
  for (const p of chain) {
    const card = makeProfileCard(p);
    chainCards.append(card);
    profileEls[p.name] = card;
  }
  for (const p of solo) {
    const card = makeProfileCard(p);
    soloCards.append(card);
    profileEls[p.name] = card;
  }
  bar.append(chainCol, soloCol);
  paintProfiles();
}
// Paint each card: state class (off/full/hollow = INTENT) + present-count
// (MACHINE TRUTH). Cards are inert while scanning or a run is on.
function paintProfiles() {
  const busy = applyRunning || scanning;
  for (const [name, card] of Object.entries(profileEls)) {
    // Card visual = MY activation (not coverage), so the click (toggle own
    // activation) always matches what the card shows — fixes the "can't deselect
    // a card filled indirectly" bug. Three states (spec §21b):
    //   active   — I clicked it (in activeProfiles)
    //   indirect — NOT activated, but its members are all wanted via ANOTHER
    //              active bundle → intermediate tint ("already covered")
    //   hollow   — active but a member was pulled out (incomplete)
    //   off      — none of the above
    const active = M.isProfileActive(model, name);
    const coverage = M.profileStateOf(model, name); // "off" | "full" | "hollow"
    const indirect = !active && coverage === "full";
    card.classList.toggle("full", active && coverage !== "hollow");
    card.classList.toggle("hollow", active && coverage === "hollow");
    card.classList.toggle("indirect", indirect);
    card.disabled = busy;
    const sw = card.querySelector(".pc-switch");
    if (sw) {
      sw.classList.toggle("on-in", active && coverage !== "hollow");
      sw.classList.toggle("mixed", (active && coverage === "hollow") || indirect);
      sw.classList.toggle("on-out", !active && !indirect);
    }
    // "installed / wanted" — progress toward what this bundle will put down.
    // Blank when the bundle wants nothing (inactive) so an off card stays quiet.
    const { present, total } = M.profileProgress(model, name);
    const countEl = card.querySelector(".pc-count");
    if (countEl) {
      countEl.textContent = total ? `${present}/${total}` : "";
      countEl.title = total ? `${present} installed of ${total} this bundle wants` : "";
    }
  }
}
// Click rule (bundle-driven, spec Consolidation §2): a bundle card is a TOGGLE.
// Clicking an inactive one applies it (pulls its packages in); clicking an active
// one removes it (its pull vanishes). §3 composition: a package shared with
// another active bundle survives (removeProfile only drops THIS bundle's pull);
// a manual per-package "out" still wins. (Reset clears all active bundles.)
function applyProfileClick(name) {
  if (M.isProfileActive(model, name)) M.removeProfile(model, name);
  else M.applyProfile(model, name);
  repaintAll();
  persistSelection();
}

// The sliding switch, THREE positions: knob left = out (refuse), centre = auto
// (follow the bundles), right = in (want). POSITION is the user's hand; COLOUR
// (green/red vivid) is overlaid by paintPkg from the computed action, so `auto`
// (centre) can still glow green (a bundle will install it) or red (present, no
// longer wanted). See spec §21b. Click cycles out → auto → in → out.
function makeToggle() {
  const el = document.createElement("span");
  el.className = "toggle mixed";
  el.innerHTML = '<span class="knob"></span>';
  return el;
}
// The next state in the out → auto → in → out cycle.
function nextToggleState(manual) {
  if (manual === "out") return null; // out → auto
  if (manual === null || manual === undefined) return "in"; // auto → in
  return "out"; // in → out
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
  const locked = M.isLocked(model, i);
  const el = r.chk;
  // Two layers (spec §21b): POSITION = the raw manual hand — knob left (out),
  // centre (auto/null), right (in). COLOUR = the computed diff (actClass from
  // actionOf) laid over the track, so centre/auto can glow green/red.
  const manual = M.manualToggle(model, i); // "in" | "out" | null(auto)
  const pos = manual === "in" ? "on-in" : manual === "out" ? "on-out" : "mixed";
  el.className = "toggle " + pos + (locked ? " locked" : "") + actClass(i);
  el.style.cursor = locked ? "not-allowed" : "pointer";
  el.title = locked
    ? "Locked by the author"
    : "left = refuse · centre = auto (follow bundles) · right = want";
  // A row whose desired state is absent reads dimmer (not wanted).
  const wanted = M.desiredOf(model, i) === "present";
  r.details.classList.toggle("row-out", !wanted);
  // `.want` = wanted (drives the Bundles-tab scope). `.bundle-member` = pulled by
  // an active bundle, EVEN IF a manual "out" overrides it — so an un-checked
  // member stays visible (greyed) in the Bundles tab and the bundle reads
  // "incomplete" (hollow) instead of the row silently vanishing. Spec Cons §3.
  r.details.classList.toggle("want", wanted);
  r.details.classList.toggle("bundle-member", M.inActiveProfile(model, i));
  // `.will-remove` = a plain Apply WOULD uninstall this (present + not wanted, or
  // vetoed). It must be VISIBLE in the Bundles tab (the action view): removals
  // belong to the diff as much as installs. Without this the row is hidden there
  // and the destruction is invisible until it happens. Spec: the action tab shows
  // what Apply does — both sides.
  r.details.classList.toggle("will-remove", M.actionOf(model, i) === "uninstall");
  // `.will-change` = a plain Apply WOULD act on this row (install/update/remove).
  // Drives the Bundles-tab "diff only" mode: by default show only what changes,
  // with a toggle to reveal everything.
  r.details.classList.toggle("will-change", M.isActionable(model, i));
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
  paintProfiles(); // a manual toggle can make a profile go hollow (or full again)
  refreshLiveness();
  if (!opts.silent) persistSelection();
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
// macOS "App Management" permission status, set once at launch from the `plan`
// message: "granted" | "missing" | "na" (na = not macOS or not applicable). Drives the
// banner (missing only), the settings pill, and the on-focus re-check.
let appmgmtStatus = "na";
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

// The re-scan veil covers the WHOLE window (position:fixed, top-level), so the
// class rides <body> — not #steps, whose containing block used to trap it over the
// package list while the Apply bar, bundle cards, tabs and header stayed crisp and
// clickable during the scan.
//
// ⚠️ ONE pair of functions for every site, because a full-screen veil that STICKS is
// far worse than a partial one: the window would be unusable, not just misleading.
// Every clear path (apply-plan · done · socket close) calls hideRescanVeil().
// `applyRunning` + refreshLiveness() still disable the buttons independently — the
// veil communicates, the disable enforces. Keep both.
function showRescanVeil() {
  document.body.classList.add("steps-refreshing");
}
function hideRescanVeil() {
  document.body.classList.remove("steps-refreshing");
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
  // The server re-scans presence before it replies with the plan. No longer a
  // SILENT gap: it narrates each re-probed package (`rescan-progress`). The veil
  // covers the WHOLE window so nothing crisp invites a click we can't honour.
  // Cleared on `apply-plan` (focus mode takes over), `done` (nothing to do, no
  // plan) or a dropped socket.
  resetRefreshProgress(); // open on the resting frame, not the last run's tail
  showRescanVeil();
  // Focus mode engages on the server's `apply-plan` reply (it computes the plan),
  // not here — so we show the exact set of steps that will run.
  ws.send(JSON.stringify({ type: "apply", on, off, scope: scopeIdx }));
}

function render(steps, profiles = [], columns = 2) {
  M.loadPlan(model, steps, profiles);
  // The scan starts now and won't settle until `state-done`. Freeze actions and
  // dim the panel until then: rows show "checking…", nothing is clickable, and
  // each lights up as its probe answers — no acting on an incomplete plan.
  scanning = true;
  stepsEl.classList.add("scanning");
  renderProfiles(profiles, columns);
  // FLATTEN (spec §2): no bundle accordion. Rows render flat into #steps below.
  // Bundle cards on top (needs) live in #profiles (renderProfiles).
  // Order rows by primary category then name, and inject a category header before
  // each group. The Catalog tab shows the headers (grouped-by-category view, spec
  // §22); the Bundles tab hides them (it filters to wanted/will-remove rows). Rows
  // are keyed by s.i, so reordering the DOM is safe.
  const primaryCat = (s) => (Array.isArray(s.categories) && s.categories[0]) || "misc";
  const orderedSteps = [...steps].sort((a, b) =>
    primaryCat(a).localeCompare(primaryCat(b)) || a.name.localeCompare(b.name)
  );
  let lastCat = null;
  for (const s of orderedSteps) {
    const cat = primaryCat(s);
    if (cat !== lastCat) {
      const h = document.createElement("div");
      h.className = "cat-header";
      h.dataset.cat = cat;
      h.innerHTML = `<span class="cat-caret">▾</span><span class="cat-name">${cat}</span>`;
      // Accordion (Catalog only): click folds/unfolds this category's rows. We
      // toggle a `.cat-collapsed` set tracked on the header + hide its rows by
      // data-cat. Rows stay in the DOM (Bundles tab still sees them flat).
      h.onclick = () => {
        const collapsed = h.classList.toggle("cat-collapsed");
        stepsEl.querySelectorAll("details[data-cat]").forEach((row) => {
          if (row.dataset.cat === cat) row.classList.toggle("cat-hidden", collapsed);
        });
      };
      stepsEl.append(h);
      lastCat = cat;
    }
    const d = document.createElement("details");
    d.dataset.cat = cat; // for the Catalog accordion (fold by category)
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
      setToggle(s.i, nextToggleState(M.manualToggle(model, s.i))); // cycle out→auto→in
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
    const cats = Array.isArray(s.categories) && s.categories.length
      ? ` <span class="cat-tag">${s.categories.join(" · ")}</span>`
      : "";
    name.innerHTML = `${s.name}` + cats + profTags +
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
    // Wanting is now the ✓ cell of the segmented toggle (spec §20) — no separate
    // Add button. "My extras" = the packages set to ✓ (manualToggle "in").
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
    stepsEl.append(d);
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
// While focus mode holds, the rows of the plan are also REORDERED to the order they
// will actually run in (topo_sort put the required before its dependents, so `jj`
// comes before `jj skills` even if the catalogue lists them the other way). Read
// top-to-bottom, the screen then IS the running order.
//
// Only for the duration of the Apply: at rest the list keeps its catalogue order —
// that's the map (stable, the same every time you open it), and the map must not
// move. During the Apply the screen is a story instead, and the story follows time.
// `planOrigin` remembers where each moved row came from, to put it back on done.
let planOrigin = null;
function enterFocusMode(planIndices) {
  document.body.classList.add("applying");
  const inPlan = new Set(planIndices);
  for (const i of Object.keys(rows).map(Number)) {
    // focus-show = this row is part of the plan → stays visible throughout.
    rows[i].details.classList.toggle("focus-show", inPlan.has(i));
  }
  // Remember each planned row's original slot (parent + the node it sat before),
  // then re-append them in plan order. Only planned rows move; the hidden ones
  // stay where they are, so restoring is exact.
  planOrigin = planIndices
    .map((i) => rows[i]?.details)
    .filter(Boolean)
    .map((el) => ({ el, parent: el.parentNode, before: el.nextSibling }));
  for (const i of planIndices) {
    const el = rows[i]?.details;
    if (el) el.parentNode.appendChild(el); // re-append = move to the end, in plan order
  }
}
function exitFocusMode() {
  document.body.classList.remove("applying");
  // Put the moved rows back where the catalogue had them (reverse order, so each
  // `before` anchor is still valid when its turn comes).
  if (planOrigin) {
    for (const { el, parent, before } of planOrigin.slice().reverse()) {
      parent.insertBefore(el, before);
    }
    planOrigin = null;
  }
  // Reveal everything again; leave the per-row open/fold state as setStatus left
  // it (failures open, successes folded) — the failures thus stand out on return.
  for (const i of Object.keys(rows).map(Number)) {
    rows[i].details.classList.remove("focus-show");
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
const fbAttemptEl = document.getElementById("fb-attempt");
const fbStopBtn = document.getElementById("fb-stop");
let fbActive = null;
let fbActiveUrl = null; // url the modal's "Open blocked page" step acts on

// The stepper's amber highlight rides the CURRENT step, so the primary action is
// never stale. "open" → step 1 lit; "retry" → step 1 done (green ✓), step 3 lit.
// The passive step 2 never lights (nothing to click in Talos). `null` lights
// nothing — used when retries run out and the stepper has no action left to offer.
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
  // "Stop the whole Apply" only appears while the Apply is actually held — outside
  // a run there is no plan to abandon. Three outcomes, three controls, no synonyms.
  r.fbBanner.innerHTML =
    `<b>⚠ A download was blocked by the corporate firewall (403).</b><br>` +
    `Open the blocked page and approve the access request — you don't need to ` +
    `download anything. Then Retry and Talos fetches it for you.` +
    `<div class="fb-attempt" data-fb="attempt" hidden></div>` +
    `<div class="fb-banner-actions">` +
    (url ? `<button data-fb="open">Open blocked page</button>` : "") +
    `<button data-fb="retry">Retry</button>` +
    `<button data-fb="giveup">Skip this one</button>` +
    `<button data-fb="stop" hidden>Stop the whole Apply</button></div>`;
  r.fbBanner.classList.add("show");
  r.details.open = true;
  const openBtn = r.fbBanner.querySelector('[data-fb="open"]');
  if (openBtn) openBtn.onclick = () => openForbidden(url);
  r.fbBanner.querySelector('[data-fb="retry"]').onclick = () => retryStep(i);
  r.fbBanner.querySelector('[data-fb="giveup"]').onclick = () => giveUp(i);
  r.fbBanner.querySelector('[data-fb="stop"]').onclick = () => stopApply(i);
  // Transient modal — the guided stepper for the active block.
  fbActive = i;
  fbActiveUrl = url;
  renderLink(fbLink, url);
  // With a URL: step 1 (Open) is the live action. Without one (rare — nothing
  // extractable): step 1 can't act, so disable it and start the highlight on
  // Retry (the user clears access however they can, then retries).
  fbStepOpen.disabled = !url;
  setFbStep(url ? "open" : "retry");
  // Fresh block → fresh attempt state. `forbidden-pause` refines both a beat later
  // (it carries the count and whether retries remain); a `forbidden` outside a run
  // never pauses, so Stop stays hidden until a pause says a run is held.
  fbAttemptEl.hidden = true;
  fbAttemptEl.textContent = "";
  fbStepRetry.hidden = false;
  fbStopBtn.hidden = true;
  // Reset the "More info…" disclosure each time: link hidden, prompt shown (only
  // if there's a URL to reveal).
  fbLinkWrap.hidden = true;
  fbMoreInfo.classList.toggle("empty", !url);
  fbEl.classList.add("show");
}

// Says WHICH attempt just failed, and withdraws Retry once the server stops
// offering it. Without the count, a second identical banner reads as "my click did
// nothing" — the retry ran, it just hit the same wall. Only shown from attempt 2:
// on the first one there is no history to report.
function paintFbAttempt(i, attempt, canRetry) {
  const r = rows[i];
  const note =
    attempt < 2
      ? ""
      : canRetry
        ? `Attempt ${attempt} was blocked too — the approval may not have gone through yet.`
        : `Attempt ${attempt} was blocked too. No more retries for this one: skip it, or stop the Apply.`;
  if (r && r.fbBanner) {
    const el = r.fbBanner.querySelector('[data-fb="attempt"]');
    if (el) {
      el.textContent = note;
      el.hidden = !note;
    }
    const retryBtn = r.fbBanner.querySelector('[data-fb="retry"]');
    if (retryBtn) retryBtn.hidden = !canRetry;
    // Abandoning the run is only meaningful while a run is held.
    const stopBtn = r.fbBanner.querySelector('[data-fb="stop"]');
    if (stopBtn) stopBtn.hidden = false;
  }
  if (fbActive === i) {
    fbAttemptEl.textContent = note;
    fbAttemptEl.hidden = !note;
    fbStepRetry.hidden = !canRetry;
    fbStopBtn.hidden = false;
    if (!canRetry) setFbStep(null); // nothing left to light — the primary action is gone
  }
}

// Drops the TRANSIENT modal only. The row banner is the durable surface, so it
// stays: a retry that is still running has not settled anything yet, and if it is
// blocked again the banner is where the next attempt is reported.
function dismissFbModal() {
  fbEl.classList.remove("show");
  fbActive = null;
  fbActiveUrl = null;
}

function clearForbidden(i) {
  const r = rows[i];
  if (r && r.fbBanner) {
    r.fbBanner.classList.remove("show");
    r.fbBanner.innerHTML = "";
  }
  if (fbActive === i) dismissFbModal();
}

// True while the server holds the Apply on a 403, waiting for us to decide. The
// blocked step has ALREADY finished (its exit code is in); nothing else starts
// until we answer, so the user can go unblock the page without installs scrolling
// past behind them — and without the following packages burning on the same
// blocked source.
let fbPaused = false;
// Whether the server still offers a retry for the held row (it caps them, so a
// user cannot hang the Apply forever against a firewall that will not budge).
let fbCanRetry = true;

// Answer the server's `forbidden-pause`. THREE outcomes, three distinct messages
// (decision.js owns the mapping so the words and the wire cannot drift):
//   retry    → the SAME action runs again on this row, then the plan carries on
//   continue → skip this row, run the rest
//   stop     → abandon everything still to do
// Answering is gated on a DECISION, not on success: the user may have failed to
// unblock, and that is still their call.
function decideForbidden(gesture) {
  const type = forbiddenMessage(gesture, fbPaused);
  if (!type) return false;
  fbPaused = false;
  ws.send(JSON.stringify({ type }));
  overall.textContent =
    gesture === "retry" ? "retrying…" : gesture === "stop" ? "stopping…" : "applying…";
  return true;
}

function retryStep(i) {
  // Paused mid-Apply: the loop is holding for us, so ask it to RE-RUN this row.
  // The banner stays until the retry's own verdict lands (a `step` event settles
  // it) — clearing it now would claim success we don't have yet.
  if (fbPaused) {
    if (!fbCanRetry) return; // cap reached: only continue/stop remain
    // The row's live status comes from the server's `step` event, which lands as
    // soon as the retry starts — painting one here would only risk disagreeing.
    // The modal goes (it would cover the run it just started); the row banner
    // stays, because nothing has been settled yet.
    if (fbActive === i) dismissFbModal();
    decideForbidden("retry");
    return;
  }
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
  // "Give up on THIS row" — not on the run. Paused mid-Apply: release the loop so
  // the rest of the plan goes through. Either way the row KEEPS the status the
  // server gave it (`forbidden` → "blocked by firewall"): we used to paint `fail`
  // locally, which the journal and any later `state` disagreed with. Giving up
  // drops the recovery UI, it does not rewrite what happened.
  clearForbidden(i);
  decideForbidden("continue");
}

// Abandon the whole Apply. The server has always implemented this; until now
// nothing in the UI could reach it, so "abandon the rest of the plan" was a
// promise made only in a comment.
function stopApply(i) {
  clearForbidden(i);
  decideForbidden("stop");
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
document.getElementById("fb-stop").onclick = () => {
  if (fbActive !== null) stopApply(fbActive);
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
  // The card's X/Y count (profileProgress) reads machine PRESENCE, which just
  // changed — repaint the cards so the count tracks the scan, not only manual
  // toggles. Without this the count stayed stale until the next card/row click.
  paintProfiles();
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
// "Show all" — a UI preference (like advanced): the Bundles tab shows only
// changing rows by default (the diff); flip this to reveal everything the active
// bundles select. Persisted in selection.json's `ui` (one config file for all),
// via persistSelection; restored by applySavedSelection from the plan.
const showAllToggle = document.getElementById("show-all");
function applyShowAll(on) {
  document.body.classList.toggle("show-all", on);
  showAllToggle.checked = on;
}
showAllToggle.onchange = (e) => {
  applyShowAll(e.target.checked);
  persistSelection();
};
applyShowAll(false);

// --- theme gallery: a UI pref like advanced/show-all. Empty name = the :root
// default (Tokyo Night); any other sets data-theme on <html>. Persisted in the
// same selection.json ui block, restored by applySavedSelection. ---
function applyTheme(name) {
  const t = name || "";
  if (t) document.documentElement.dataset.theme = t;
  else delete document.documentElement.dataset.theme;
  for (const pill of document.querySelectorAll(".theme-pill")) {
    pill.classList.toggle("active", (pill.dataset.themeName || "") === t);
  }
}
for (const pill of document.querySelectorAll(".theme-pill")) {
  pill.onclick = () => {
    applyTheme(pill.dataset.themeName || "");
    persistSelection();
  };
}
applyTheme("");

// --- tabs (Bundles / Catalog / Log) ---
// Bundles and Catalog share ONE DOM (#view-bundles holds #steps); they differ
// only by body scope class (tab-bundles filters to .want rows, tab-catalog shows
// all — see the CSS). Only the Log tab swaps to a different view container.
function setTab(v) {
  document.querySelectorAll(".tab").forEach((t) =>
    t.classList.toggle("active", t.dataset.view === v)
  );
  const isLog = v === "log";
  document.getElementById("view-bundles").classList.toggle("active", !isLog);
  document.getElementById("view-log").classList.toggle("active", isLog);
  document.body.classList.toggle("tab-bundles", v === "bundles");
  document.body.classList.toggle("tab-catalog", v === "catalog");
  if (isLog) ws.send(JSON.stringify({ type: "get-log" }));
}
document.querySelectorAll(".tab").forEach((tab) => {
  tab.onclick = () => setTab(tab.dataset.view);
});
// Default scope: the Bundles tab (only wanted rows) is the active tab at load.
document.body.classList.add("tab-bundles");

// --- advanced mode: a UI preference (per-package toggles, postures, per-bundle
// Apply). Default OFF → simple. Persisted in selection.json's `ui` (one config
// file for all), sent via persistSelection on change; restored by
// applySavedSelection from the plan. Boots OFF until the plan arrives.
const advToggle = document.getElementById("advanced-toggle");
function applyAdvanced(on) {
  document.body.classList.toggle("advanced", on);
  advToggle.checked = on;
}
advToggle.onchange = (e) => {
  applyAdvanced(e.target.checked);
  persistSelection();
};
applyAdvanced(false);

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

// --- macOS App Management permission: banner + settings pill + gate sheet ---
// Paint the settings pill (granted=green, missing=red, na=neutral) and show the
// top-of-list banner ONLY when the permission is missing.
function renderAppmgmt() {
  const pill = document.getElementById("appmgmt-pill");
  const banner = document.getElementById("appmgmt-banner");
  const map = { granted: ["granted", "ok"], missing: ["missing", "fail"], na: ["—", ""] };
  const [label, cls] = map[appmgmtStatus] || map.na;
  if (pill) {
    pill.textContent = label;
    pill.className = "badge " + cls;
  }
  if (banner) banner.hidden = appmgmtStatus !== "missing";
}
const openAppmgmtSettings = () =>
  ws.send(JSON.stringify({ type: "open-appmgmt-settings" }));
document.getElementById("appmgmt-open")?.addEventListener("click", openAppmgmtSettings);
document.getElementById("appmgmt-banner-open")?.addEventListener("click", openAppmgmtSettings);
document.getElementById("appmgmt-sheet-open")?.addEventListener("click", openAppmgmtSettings);
document.getElementById("appmgmt-sheet-cancel")?.addEventListener("click", () => {
  document.getElementById("appmgmt-sheet")?.classList.remove("show");
});
// No in-app re-check: macOS caches the TCC verdict per PROCESS, so re-probing
// from the running Talos returns the STALE answer even after the user grants the
// permission. The system itself requires a quit-and-relaunch — that's the only
// honest signal, and it's exactly what the startup check reads. So the status is
// only ever set at launch (from the `plan` message); there is no Refresh button.

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
    // Release tag leads (the human-facing version), then the source snapshot.
    // Older backends without a tag → fall back to the pre-tag format.
    el.textContent = build.tag
      ? `${build.tag} · build ${label} — ${build.builtAt}`
      : `build ${label} — ${build.builtAt}`;
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
      // The plan is what makes the progress bar DETERMINATE: before it, we don't
      // know how many packages there are, so the bar shuttles.
      scanTotal = (msg.steps || []).length;
      scanSeen = 0;
      splashProgress(null, 0);
      render(
        msg.steps,
        msg.profiles || [],
        msg.profileColumns,
      );
      applySavedSelection(msg.selection); // restore persisted decisions (yellow)
      if (msg.consent && !msg.consent.decided) consentEl.classList.add("show"); // first boot
      if (msg.build) showBuild(msg.build); // stamp the UI with the exact source snapshot
      appmgmtStatus = msg.appmgmt || "na";
      renderAppmgmt();
      break;
    case "log":
      renderLog(msg.consent, msg.history);
      break;
    case "rescan-progress":
      // The Apply's scoped re-scan, one message per re-probed row. Distinct from
      // `state` (which carries the verdict): this fires BEFORE the verdict is known,
      // so the veil names what is being checked while it is being checked.
      refreshProgress(msg.name, msg.nth, msg.total);
      break;
    case "state": // ground truth from the machine
      // Narrate the SERIAL scan: this verdict just landed, so name the package and
      // advance i/n. Serial is what makes this honest — with a fan-out there would
      // be no single "current" package to name.
      scanSeen += 1;
      splashProgress(model.pkgs.get(msg.i)?.name, scanSeen);
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
      // and the transient modal. The page is NOT opened for them — they click
      // "Open blocked page" (the server only opens on that message). url may be
      // null (nothing extractable → link-less message).
      showForbidden(msg.i, msg.url || null);
      break;
    case "forbidden-pause":
      // The Apply is HELD on this row. The blocked step already finished; nothing
      // else will start until we answer. Say so, so the user knows they have the
      // time to go unblock the page — the whole point of pausing.
      fbPaused = true;
      fbCanRetry = msg.canRetry !== false;
      // A second identical banner reads as "nothing happened" — name the attempt,
      // and when the server stops offering retries, stop offering the button.
      paintFbAttempt(msg.i, msg.attempt || 1, fbCanRetry);
      overall.textContent = "waiting for you";
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
      hideRescanVeil(); // scan done, plan is here
      enterFocusMode((msg.plan || []).map((p) => p.i));
      break;
    case "done":
      overall.textContent = msg.nothing ? "nothing to do" : "done";
      applyRunning = false;
      fbPaused = false; // the loop is over — no pause left to release
      hideRescanVeil(); // net: scan found nothing → no apply-plan
      exitFocusMode(); // everything reappears — failures already open, stand out
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
    case "needs-appmgmt": {
      // The user tried to update an app in /Applications but the permission is
      // missing — the step did NOT run. Raise the gate sheet.
      document.getElementById("appmgmt-sheet")?.classList.add("show");
      break;
    }
  }
};
ws.onclose = () => {
  overall.textContent = "disconnected";
  hideRescanVeil(); // don't leave the veil stuck on
  hideWait(); // don't leave the banner stuck on
};
