// The action bar: the Apply button and the line beside it, from the state of the session.
// DOM-free, so every state the bar can show is a test, not a screenshot.
//
// ⭐ THE BUTTON IS SHORT AND THE LINE IS LONG. The button says one word and, under it, a few
// more; the line on its left says the rest. The button used to carry the whole sentence
// ("Apply — 2 to apply · ~4 min for 1 and 1 unknown") and grew from 60 to 240 px as rows were
// toggled, pushing Refresh and Reset along a sticky bar. It now has a floor width the common
// cases fit in, so toggling a row changes its words and not its place.
//
// ⭐ ONE FILLED BUTTON AT A TIME. Apply when there is something to send; Quit when there is
// nothing left to do — the session's natural end. `quitPrimary` is that second case.
//
// ⚠️ REMOVALS ARE NAMED ON THE BUTTON, not only in the line. Apply honours the rows you
// unchecked, and a count that hid them would be the label lying. A reinstall (a downgrade to a
// pin) removes the package first, so it is named apart for the same reason.

// One plan action → [what the line says, what the button counts], singular then plural.
const KINDS = {
  install: { line: "to install", one: "install", many: "installs" },
  upgrade: { line: "to update", one: "update", many: "updates" },
  downgrade: { line: "to reinstall", one: "reinstall", many: "reinstalls", destructive: true },
  uninstall: { line: "to remove", one: "removal", many: "removals", destructive: true },
};
const ORDER = ["install", "upgrade", "downgrade", "uninstall"];

// What a running row is doing, said as the line's first word.
const DOING = {
  installing: "Installing",
  upgrading: "Updating",
  reinstalling: "Reinstalling",
  uninstalling: "Removing",
};

// The rest of the bar's sentence when the plan is empty: what Apply does, for whoever reads
// the bar for the first time. The list below already says that there is nothing to send.
export const IDLE_HINT = "Installs what you checked, updates what's outdated, removes what you unchecked.";

const plural = (n, k) => `${n} ${n === 1 ? KINDS[k].one : KINDS[k].many}`;

// The button's second line for a plan: the destructive kinds each by name, everything else in
// one count — named after its kind when there is only one ("2 installs"), "changes" otherwise.
export function planSub(actions) {
  const n = countKinds(actions);
  const parts = [];
  const plain = ORDER.filter((k) => !KINDS[k].destructive && n[k]);
  const plainTotal = plain.reduce((s, k) => s + n[k], 0);
  if (plain.length === 1) parts.push(plural(plainTotal, plain[0]));
  else if (plain.length > 1) parts.push(`${plainTotal} changes`);
  for (const k of ORDER) if (KINDS[k].destructive && n[k]) parts.push(plural(n[k], k));
  return parts.join(" · ");
}

function countKinds(actions) {
  const n = { install: 0, upgrade: 0, downgrade: 0, uninstall: 0 };
  for (const a of actions) if (a in n) n[a] += 1;
  return n;
}

// The bar for one moment of the session.
//
//   conn      "connecting" | "open" | "closed"
//   scanning  the machine is being read; `scan` = { seen, total } when the count is known
//   running   null, or what holds the session: { kind: "apply" | "diff" | "retry",
//             word?: a transient word that overrides the progress ("stopping…"),
//             paused?: the Apply waits for an answer on a blocked row,
//             done, total: rows started / rows in the plan (0 before the plan is known),
//             current: { name, status } of the row running now, or null }
//   actions   the plan's actions, one per row the Apply would run
//   estimate  the duration phrase for that plan ("" when there is none)
//
// Returns { label, sub, live, disabled, hint: [{ text, tone }], quitPrimary }. `tone` is
// "strong", "remove" or undefined (the line's ordinary dim).
export function barView({ conn, scanning, scan, running, actions = [], estimate = "" }) {
  const view = { label: "Apply", sub: "", live: false, disabled: true, hint: [], quitPrimary: false };
  if (conn !== "open") {
    view.sub = conn === "closed" ? "disconnected" : "connecting…";
    view.hint = [{ text: conn === "closed" ? "Lost the connection to the engine." : "Connecting to the engine…" }];
    return view;
  }
  if (running) return runningView(view, running);
  if (scanning) {
    view.sub = "scanning…";
    const count = scan && scan.total > 0 ? ` — ${Math.min(scan.seen, scan.total)} of ${scan.total} packages` : "";
    view.hint = [{ text: `Reading this machine${count}…` }];
    return view;
  }
  if (actions.length === 0) {
    view.sub = "nothing to do";
    view.hint = [{ text: IDLE_HINT }];
    view.quitPrimary = true;
    return view;
  }
  view.sub = planSub(actions);
  view.live = true;
  view.disabled = false;
  const n = countKinds(actions);
  view.hint = ORDER.filter((k) => n[k]).map((k) => ({
    text: `${n[k]} ${KINDS[k].line}`,
    tone: KINDS[k].destructive ? "remove" : "strong",
  }));
  if (estimate) view.hint.push({ text: estimate });
  return view;
}

function runningView(view, run) {
  if (run.paused) {
    view.label = "Paused";
    view.sub = "waiting for you";
    view.hint = [{ text: "A blocked row is waiting for your answer." }];
    return view;
  }
  view.label = run.kind === "diff" ? "Diffing" : run.kind === "retry" ? "Retrying" : "Applying";
  const progress = run.total > 0 ? `${Math.max(1, Math.min(run.done, run.total))} of ${run.total}` : "";
  view.sub = run.word || progress || "starting…";
  const cur = run.current;
  if (cur && DOING[cur.status]) {
    view.hint = [{ text: `${DOING[cur.status]} ${cur.name}`, tone: "strong" }];
    if (progress) view.hint.push({ text: progress });
    // The one interruption there is lives on the row itself: say where.
    view.hint.push({ text: "Stop is on its row" });
  } else {
    view.hint = [{ text: run.kind === "apply" && !progress ? "Checking this machine before acting…" : "Working…" }];
  }
  return view;
}
