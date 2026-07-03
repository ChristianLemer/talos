// app.js — the Talos panel UI. Pure decision logic lives in decision.js
// (imported below), shared with the server so the rule can never drift.
import { resolvePosture, desiredState as _desiredState, actionFor, isLockedPosture } from "./decision.js";

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
  setTimeout(() => { qi = (qi + 1) % QUIPS.length; quipEl.textContent = QUIPS[qi]; quipEl.style.opacity = "1"; }, 300);
}, 2200);
let splashDone = false;
function hideSplash() {
  if (splashDone) return; splashDone = true;
  clearInterval(quipTimer);
  // keep it up a beat so the centaur is seen even if detection is instant
  setTimeout(() => { splash.classList.add("hide"); setTimeout(() => splash.remove(), 450); }, 400);
}
setTimeout(hideSplash, 12000);   // safety: never let the splash stick forever

// Shorter terminals (10 rows) — they scroll, and tall fixed boxes waste
// vertical space on small screens. Smaller font on short viewports.
const TERM_ROWS = window.innerHeight < 700 ? 8 : 12;
const newTerm = () => new Terminal({
  cols:100, rows:TERM_ROWS, fontSize: window.innerHeight < 700 ? 11 : 12, convertEol:false,
  theme:{ background:"#1a1b26", foreground:"#c0caf5" },
});

const GLYPH = { waiting:"·", installing:"⠹", uninstalling:"⠹", upgrading:"⠹", ok:"✓", absent:"·", fail:"✗" };
const LABEL = { waiting:"absent", installing:"installing…", uninstalling:"removing…", upgrading:"updating…", ok:"present", absent:"removed", fail:"failed" };

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
const PILLWORD = { auto:"auto", on:"opt-in", off:"opt-out", mandatory:"mandatory", forbidden:"forbidden" };
// A locked posture shows its own word; else the override word.
function pillFor(i) {
  const p = rows[i] && rows[i].posture;
  if (p === "mandatory" || p === "forbidden") return { word: p, cls: "lock", locked: true };
  const d = ownDecision(i);
  return { word: PILLWORD[d], cls: d === "on" ? "yes" : d === "off" ? "no" : "auto", locked: false };
}
function paintPill(el, word, cls, extra) {
  el.textContent = word;
  el.className = `pill ${cls}${extra ? " " + extra : ""}`;
}
const decision = {};             // package index i → "auto" | "on" | "off" (override; opt-* only)
const bundleEls = {};            // bundle name → { …, decision, pkgs:[i], chk }
// Cycle depends on posture: opt-in/opt-out cycle auto→on→off→auto;
// mandatory/forbidden don't cycle (locked).
const CYCLE3 = { auto:"on", on:"off", off:"auto" };
const ICON_COPY = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/></svg>';
const ICON_OK = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';
const CYCLE = { auto:"on", on:"off", off:"auto" };

// --- persistence: only DECIDED (yellow) choices are saved; auto is not.
// Saved by NAME (bundle::package), not index, so it survives bundles being
// added/reordered. Sent to the server, which writes it next to consent.
let stepNames = {};              // i → "bundle::name" (stable key)
let bundleNameOf = {};           // bundle name is its own key
function persistSelection() {
  const pkgs = {}; const bundlesSel = {};
  for (const i of Object.keys(decision)) {
    if (decision[i] && decision[i] !== "auto" && stepNames[i]) pkgs[stepNames[i]] = decision[i];
  }
  for (const [name, be] of Object.entries(bundleEls)) {
    if (be.decision && be.decision !== "auto") bundlesSel[name] = be.decision;
  }
  ws.send(JSON.stringify({ type: "set-selection", selection: { pkgs, bundles: bundlesSel } }));
}
function applySavedSelection(sel) {
  if (!sel) return;
  const byName = {};
  for (const [i, key] of Object.entries(stepNames)) byName[key] = +i;
  for (const [key, state] of Object.entries(sel.pkgs || {})) {
    if (byName[key] != null && (state === "on" || state === "off")) setDecision(byName[key], state, { silent: true });
  }
  for (const [name, state] of Object.entries(sel.bundles || {})) {
    if (bundleEls[name] && (state === "on" || state === "off")) setBundleDecision(name, state, { silent: true });
  }
}

// Own override the user set on THAT row (opt-* only); default auto.
function ownDecision(i) { return decision[i] || "auto"; }
function isLocked(i) { return isLockedPosture(rows[i] && rows[i].posture); }
// Effective override = own if set, else inherited from the bundle's pill.
function effectiveDecision(i) {
  const own = ownDecision(i);
  if (own !== "auto") return own;
  const r = rows[i]; const be = r && bundleEls[r.bundle];
  return be ? (be.decision || "auto") : "auto";
}
// DESIRED state (present|absent) — delegates to the SHARED rule so the front and
// the server resolve posture+override identically (no drift).
function desiredState(i) {
  return _desiredState(rows[i] && rows[i].posture, effectiveDecision(i));
}

function paintPkg(i) {
  const r = rows[i]; if (!r) return;
  const pf = pillFor(i);
  paintPill(r.chk, pf.word, pf.cls);
  r.chk.classList.toggle("locked", pf.locked);
  r.chk.style.cursor = pf.locked ? "not-allowed" : "pointer";
}
function setDecision(i, state, opts = {}) {
  if (isLocked(i)) return;                 // author's posture wins — no override
  decision[i] = state;
  paintPkg(i);
  refreshLiveness();
  if (!opts.silent) persistSelection();
}
function cyclePkg(i) { if (!isLocked(i)) setDecision(i, CYCLE3[ownDecision(i)]); }

// A bundle is LOCKED (no cyclable pill) if it isn't selectable, or if none
// of its packages can be overridden (all mandatory/forbidden) — then there's
// nothing to decide at the bundle level.
function bundleLocked(name) {
  const be = bundleEls[name]; if (!be) return true;
  if (!be.selectable) return true;
  return be.pkgs.length > 0 && be.pkgs.every((i) => isLocked(i));
}
// Bundle pill: cascades an override onto its CHANGEABLE packages (opt-*),
// leaving mandatory/forbidden locked (the author's posture always wins).
function setBundleDecision(name, state, opts = {}) {
  const be = bundleEls[name]; if (!be) return;
  if (bundleLocked(name)) return;          // locked bundle → no override
  be.decision = state;
  paintPill(be.chk, PILLWORD[state], state === "on" ? "yes" : state === "off" ? "no" : "auto");
  be.head.classList.toggle("on", state === "on");
  be.pkgs.forEach(paintPkg);   // repaint: unlocked packages may now inherit
  refreshLiveness();
  if (!opts.silent) persistSelection();
}
function cycleBundle(name) {
  if (bundleLocked(name)) return;
  const be = bundleEls[name]; if (!be) return;
  setBundleDecision(name, CYCLE3[be.decision || "auto"]);
}
// Paint a bundle's pill: locked bundles show a dashed grey "locked" pill and
// don't cycle; else show their own override (auto by default).
function refreshBundleChk(name) {
  const be = bundleEls[name]; if (!be) return;
  if (bundleLocked(name)) {
    paintPill(be.chk, "locked", "lock");
    be.chk.classList.add("locked");
    be.chk.style.cursor = "not-allowed";
    be.chk.title = be.selectable ? "All packages here are fixed by the author" : "This bundle is always on";
    return;
  }
  if ((be.decision || "auto") !== "auto") return;
  paintPill(be.chk, PILLWORD.auto, "auto");
}

// Would applying package `i` actually DO something, given its EFFECTIVE
// decision? on & absent → install; on & present & outdated → upgrade;
// off & present → remove. auto → never (left as the machine has it).
// The action clicking a row's button does — it ALWAYS inverts the current
// state: absent → install, present → uninstall, present-but-stale → update
// (the useful move, not a pointless remove). null only when present and
// un-uninstallable. This is the row's immediate manual actuator.
function buttonAction(i) {
  const r = rows[i]; if (!r) return null;
  if (r.present && r.outdated) return { verb: "update", dir: "add", type: "upgrade" };
  if (r.present) return r.canUninstall ? { verb: "uninstall", dir: "remove", type: "uninstall" } : null;
  return { verb: "install", dir: "add", type: "install" };
}
// Would a plain Apply act here? Delegates to the SHARED rule (actionFor) — the
// exact same call the server makes — so the button's green/red preview always
// matches what Apply will really do. Lights the button; the label above still
// shows the manual invert action regardless.
function isActionable(i) {
  const r = rows[i]; if (!r) return false;
  return actionFor(desiredState(i), { present: r.present, outdated: r.outdated, canUninstall: r.canUninstall }) != null;
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
    const act = buttonAction(i);                       // always the invert action (label)
    const inPlan = isActionable(i);                    // would a plain Apply act here?
    const show = !!act && !applyRunning;
    // Colour follows the PLAN: directional green/red only if Apply will act
    // now; otherwise blue (available manual escape hatch, no pending change).
    r.apply.classList.toggle("live", show && inPlan);
    r.apply.classList.toggle("add", show && inPlan && act.dir === "add");
    r.apply.classList.toggle("remove", show && inPlan && act.dir === "remove");
    r.apply.classList.toggle("avail", show && !inPlan);
    r.apply.textContent = act ? act.verb : "—";        // button IS the action; — if none possible
    r.apply.disabled = applyRunning || !act;
  }
  // Bundle buttons: live if any of their packages would act.
  for (const be of Object.values(bundleEls)) {
    const live = be.pkgs.some(isActionable);
    be.apply.classList.toggle("live", live && !applyRunning);
    be.apply.disabled = applyRunning;
    // Preview the plan AT REST: a bundle with pending actions opens, a stable
    // one folds. Only when idle — during a run the execution logic (setStatus)
    // owns open/close, and we never fight the user's manual toggle mid-run.
    if (!applyRunning) be.details.open = live;
  }
  // Global button: live if anything anywhere would act.
  const g = document.getElementById("install-all");
  g.classList.toggle("live", ids.some(isActionable) && !applyRunning);
  g.disabled = applyRunning;
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
    if (desiredState(i) === "present") on.push(i); else off.push(i);
  }
  applyRunning = true; refreshLiveness();   // lock every Apply button during the run
  overall.textContent = "applying…";
  ws.send(JSON.stringify({ type:"apply", on, off, scope: scopeIdx }));
}

function render(bundles, steps) {
  for (const b of bundles) {
    const bd = document.createElement("details");
    bd.className = "bundle";
    const bsum = document.createElement("summary");
    bsum.className = "bundle-head";
    const chk = document.createElement("span");
    chk.className = "pill auto"; chk.textContent = "auto";
    chk.title = "Cycle this bundle: auto → opt-in → opt-out";
    chk.onclick = (e) => {
      e.preventDefault(); e.stopPropagation();    // cycle without folding
      cycleBundle(b.name);
    };
    const ico = document.createElement("span"); ico.className = "ico"; ico.textContent = b.emoji || "📦";
    const ct = document.createElement("span"); ct.className = "ct";
    ct.innerHTML = `<span class="cn">${b.name}</span><span class="cd">${b.description || ""}</span>`;
    const bst = document.createElement("span"); bst.className = "bstatus";
    const bapply = document.createElement("button");
    bapply.className = "applybtn"; bapply.textContent = "Apply";
    bapply.title = "Apply just this bundle";
    bapply.onclick = (e) => {
      e.preventDefault(); e.stopPropagation();   // act without folding the accordion
      applyScoped(bundleEls[b.name].pkgs);
    };
    bsum.append(chk, ico, ct, bst, bapply);
    const body = document.createElement("div"); body.className = "bundle-body";
    bd.append(bsum, body);
    stepsEl.append(bd);
    bundleEls[b.name] = { details: bd, head: bsum, status: bst, chk, apply: bapply, pkgs: [], active: 0, failed: false, decision: "auto", selectable: b.selectable !== false };
  }

  for (const s of steps) {
    const be = bundleEls[s.bundle];
    const body = be ? be.details.querySelector(".bundle-body") : stepsEl;
    if (be) be.pkgs.push(s.i);
    stepNames[s.i] = `${s.bundle}::${s.name}`;   // stable key for persistence
    const d = document.createElement("details");
    const sum = document.createElement("summary");
    const chk = document.createElement("span");
    chk.className = "pill auto"; chk.textContent = "auto";
    chk.title = "Cycle: auto (follow the posture) → opt-in → opt-out";
    chk.onclick = (e) => { e.preventDefault(); e.stopPropagation(); cyclePkg(s.i); };
    const badge = document.createElement("span");
    badge.className = "badge waiting"; badge.textContent = "·";
    const name = document.createElement("span");
    name.className = "name";
    name.innerHTML = `${s.name}` + (s.description ? ` <span class="desc">— ${s.description}</span>` : "");
    const delta = document.createElement("span");   // version delta on upgrade, e.g. 2.54 → 2.55
    delta.className = "verdelta";
    const st = document.createElement("span");
    st.className = "statusLabel waiting"; st.textContent = "absent";
    const apply = document.createElement("button");
    apply.className = "rerun"; apply.textContent = "apply"; apply.title = "Apply this one now";
    apply.onclick = (e) => {
      e.preventDefault();
      if (applyRunning) return;
      const act = buttonAction(s.i);   // send EXACTLY what the button shows
      if (!act) return;
      applyRunning = true; refreshLiveness();
      overall.textContent = "applying…";
      ws.send(JSON.stringify({ type: act.type, i:s.i }));
    };
    sum.append(chk, badge, name, delta, st, apply);
    const panel = document.createElement("div");
    panel.className = "panel";
    const copy = document.createElement("button");
    copy.className = "copy"; copy.innerHTML = ICON_COPY; copy.title = "Copy this step's output";
    copy.onclick = (e) => {
      e.preventDefault();
      const text = (rows[s.i].log || "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""); // strip ANSI
      navigator.clipboard.writeText(text).then(() => {
        copy.innerHTML = ICON_OK; setTimeout(() => copy.innerHTML = ICON_COPY, 1200);
      });
    };
    const host = document.createElement("div");
    host.className = "term-host";
    panel.append(copy, host);
    d.append(sum, panel);
    body.append(d);
    rows[s.i] = { details:d, chk, badge, statusLabel:st, delta, apply, host, term:null, log:"", bundle:s.bundle, canUninstall:s.canUninstall, posture:s.posture || "mandatory" };
    paintPkg(s.i);   // initial pill: locked word for mandatory/forbidden, else auto
  }
  // Now that every bundle knows its packages, paint bundle pills (locked or auto).
  for (const name of Object.keys(bundleEls)) refreshBundleChk(name);
}

function ensureTerm(i) {
  const r = rows[i];
  if (!r.term) { r.term = newTerm(); r.term.open(r.host); }
  return r.term;
}

const RUNNING = new Set(["installing", "uninstalling", "upgrading"]);
function setStatus(i, status) {
  const r = rows[i]; if (!r) return;
  if (RUNNING.has(status)) { r.log = ""; if (r.term) r.term.reset(); } // fresh run
  r.badge.className = "badge " + status; r.badge.textContent = GLYPH[status] || "·";
  r.statusLabel.className = "statusLabel " + status; r.statusLabel.textContent = LABEL[status] || status;
  // Track presence from the settled states, so liveness knows what's on the
  // machine. (Running states are transient — leave presence as it was.)
  if (status === "ok") { r.present = true; r.outdated = false; }  // a settled 'ok' = current now
  else if (status === "absent" || status === "waiting") { r.present = false; r.outdated = false; }
  refreshLiveness();
  if (RUNNING.has(status) || status === "fail") r.details.open = true; // show activity / failures
  if (status === "ok" || status === "absent") r.details.open = false;       // fold completed (frame stays)
  // A finished/failed row has nothing more to add to its version delta;
  // a fresh install/remove clears any stale one. (upgrade keeps it — set just before.)
  if (r.delta && status !== "upgrading") r.delta.textContent = "";
  // Bundle-level: open while working, count active packages, auto-close
  // when the bundle's last package finishes (unless something failed).
  const be = bundleEls[r.bundle];
  if (be) {
    if (RUNNING.has(status)) { be.active++; be.details.open = true; }
    if (status === "fail") { be.failed = true; be.details.open = true; }
    if (status === "ok" || status === "absent" || status === "fail") be.active = Math.max(0, be.active - 1);
    be.status.textContent = status === "installing" ? "installing…"
      : status === "uninstalling" ? "removing…" : status === "upgrading" ? "updating…" : be.failed ? "failed" : "";
    be.status.className = "bstatus " + (be.failed ? "fail" : status);
    if (be.active === 0 && !be.failed) {        // all done, all good → fold to keep the page short
      be.details.open = false;
      be.status.textContent = "";
    }
  }
}

const ws = new WebSocket(`ws://${location.host}`);

// Apply = enact your decisions (on/off) against the machine; auto is left
// alone. Server acts only on the difference. No scope → all decided packages.
document.getElementById("install-all").onclick = () => applyScoped(undefined);

// --- tabs ---
document.querySelectorAll(".tab").forEach((tab) => {
  tab.onclick = () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t === tab));
    const v = tab.dataset.view;
    document.getElementById("view-setup").classList.toggle("active", v === "setup");
    document.getElementById("view-log").classList.toggle("active", v === "log");
    if (v === "log") ws.send(JSON.stringify({ type:"get-log" }));
  };
});

// --- consent dialog + settings ---
const consentEl = document.getElementById("consent");
function setConsent(share) { ws.send(JSON.stringify({ type:"set-consent", share })); consentEl.classList.remove("show"); }
document.getElementById("consent-yes").onclick = () => setConsent(true);
document.getElementById("consent-no").onclick = () => setConsent(false);
document.getElementById("consent-toggle").onchange = (e) => ws.send(JSON.stringify({ type:"set-consent", share: e.target.checked }));
document.getElementById("clear-log").onclick = () => ws.send(JSON.stringify({ type:"clear-log" }));

function renderLog(consent, history) {
  document.getElementById("consent-toggle").checked = !!(consent && consent.share);
  const h = document.getElementById("history");
  if (!history || !history.length) { h.innerHTML = '<div class="hist-empty">No install history yet.</div>'; return; }
  h.innerHTML = "";
  for (const e of history.slice().reverse()) {     // newest first
    const row = document.createElement("div"); row.className = "hist-row";
    const when = (e.at || "").replace("T", " ").slice(0, 16);
    row.innerHTML =
      `<span class="h-when">${when}</span>` +
      `<span class="h-pkg">${e.package || ""}${e.version ? ` <span class="h-ver">${e.version}</span>` : ""}</span>` +
      `<span class="h-act ${e.ok ? "ok" : "fail"}">${e.action}${e.ok ? "" : " ✗"}</span>`;
    h.append(row);
  }
}

ws.onopen = () => overall.textContent = "checking…";
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  switch (msg.type) {
    case "plan":
      render(msg.bundles || [], msg.steps);
      applySavedSelection(msg.selection);          // restore persisted decisions (yellow)
      if (msg.consent && !msg.consent.decided) consentEl.classList.add("show"); // first boot
      break;
    case "log": renderLog(msg.consent, msg.history); break;
    case "state":                                  // ground truth from the machine
      // Detection feeds PRESENCE only (the badge/label), never the decision —
      // your yellow choices are yours. Blue auto rows just reflect the machine.
      if (rows[msg.i]) setStatus(msg.i, msg.present ? "ok" : "waiting");
      break;
    case "state-done": overall.textContent = "ready"; hideSplash(); break;
    case "outdated": {                             // a present package has a newer version
      const r = rows[msg.i]; if (!r) break;
      r.outdated = true;
      if (r.delta) r.delta.textContent = `${msg.current} → ${msg.available}`;
      refreshLiveness();                           // now this row's Apply is useful
      break;
    }
    case "step": setStatus(msg.i, msg.status); break;
    case "detail":                                 // version delta for an upgrade, e.g. "2.54→2.55"
      if (rows[msg.i] && rows[msg.i].delta) rows[msg.i].delta.textContent = (msg.detail || "").replace("→", " → ");
      break;
    case "out": {
      const text = dec.decode(b64dec(msg.data));
      ensureTerm(msg.i).write(text);
      if (rows[msg.i]) rows[msg.i].log += text;   // keep raw for the copy button
      break;
    }
    case "done":
      overall.textContent = msg.nothing ? "nothing to do" : "done";
      applyRunning = false; refreshLiveness();   // unlock; re-light what's still useful
      break;
    case "overlay":
      document.getElementById("overlay-title").textContent = msg.title || "";
      document.getElementById("overlay-body").textContent = msg.body || "";
      document.getElementById("overlay").classList.add("show");
      break;
  }
};
ws.onclose = () => overall.textContent = "disconnected";
