// finder.js — the Catalog finder's matcher. DOM-free, like model.js: app.js hands it the
// rows' words and paints what it answers; the node tests drive it directly.
//
// ⭐ A FILTER SORTS, A LABEL INFORMS. This module only ranks: it says which rows match a
// query and in which order. It never sees a row's state, scope or intention, and nothing it
// returns can change one — the finder is a view.
//
// The matching is fzf's: the query's characters must appear in the text IN ORDER, not
// necessarily side by side ("vsc" finds "Visual Studio Code"). Case and accents are folded
// on both sides ("ecran" finds "Écran"). The score rewards what a reader means by "it
// matched": a run of consecutive characters, a character that starts a word, the first
// character of the query starting one; it charges for the gaps in between.

// What a match is worth — fzf's own constants, so the ranking reads like the tool people
// already know. A matched character earns MATCH; one that starts a word earns BOUNDARY more
// (twice for the query's first character); a character right after the previous match earns
// CONSECUTIVE at least. A gap costs GAP_START once, then GAP_EXTEND per skipped character.
const MATCH = 16;
const BOUNDARY = 8;
const CONSECUTIVE = 4;
const GAP_START = -3;
const GAP_EXTEND = -1;

// The tiers. A row is ranked by the best tier its query reached, then by score: any match in
// the NAME outranks any match elsewhere, whatever the scores. The name is what a reader
// types; the rest only helps find a row whose name they forgot.
export const TIER_NAME = 0;
export const TIER_OTHER = 1;

// Lower case, accents removed: NFD splits "é" into "e" + a combining mark, the mark goes.
// ⚠️ Applied to BOTH sides, so a query typed with an accent still finds the plain spelling.
export function fold(s) {
  return String(s ?? "")
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase();
}

const isWordChar = (c) => /[\p{L}\p{N}]/u.test(c);

// Does the character at `j` start a word? The text's first character does, and so does any
// letter or digit after a separator ("code" in "visual-studio-code", "s" in "nu_plugin").
function boundary(t, j) {
  return isWordChar(t[j]) && (j === 0 || !isWordChar(t[j - 1]));
}

// The best alignment of folded `q` in folded `t`, or null when `q` is not a subsequence.
// A small dynamic programme (fzf's v2, without its back-tracking): H[j] holds the best score
// of the query so far with its last character matched AT j. Strings here are a row's name or
// a description — tens of characters — so the m×n table is a few hundred cells.
export function scoreText(q, t) {
  const m = q.length;
  const n = t.length;
  if (m === 0) return 0;
  if (m > n) return null;
  const bonus = Array.from({ length: n }, (_, j) => (boundary(t, j) ? BOUNDARY : 0));
  let prev = new Array(n).fill(-Infinity);
  for (let i = 0; i < m; i++) {
    const cur = new Array(n).fill(-Infinity);
    // The best score of the previous row ending at least two characters back, already
    // charged for the gap up to j — carried along so each cell reads it in O(1).
    let gapped = -Infinity;
    for (let j = 0; j < n; j++) {
      if (i > 0 && j >= 2) gapped = Math.max(gapped + GAP_EXTEND, prev[j - 2] + GAP_START);
      if (t[j] !== q[i]) continue;
      if (i === 0) {
        cur[j] = MATCH + 2 * bonus[j];
        continue;
      }
      const run = j >= 1 ? prev[j - 1] + MATCH + Math.max(bonus[j], CONSECUTIVE) : -Infinity;
      cur[j] = Math.max(run, gapped + MATCH + bonus[j]);
    }
    prev = cur;
  }
  const best = Math.max(...prev);
  return best === -Infinity ? null : best;
}

// A field written as prose — a description, a category — is matched WORD BY WORD: each query
// term must fall inside one word. Across a whole sentence almost any three letters are a
// subsequence ("nu" is in "ruN a commaNd for yoU"), and the finder would open every category
// for every query. A name or an id is short and chosen, so it is matched whole.
function scoreProse(q, t) {
  let best = null;
  for (const w of t.split(/[^\p{L}\p{N}]+/u)) {
    const s = scoreText(q, w);
    if (s !== null && (best === null || s > best)) best = s;
  }
  return best;
}

// What the finder reads of a row: its name, then the words that rank lower. Built once per
// plan by the caller, so a keystroke folds only the query.
export function findEntry({ name, description, categories, id }) {
  return {
    name: fold(name),
    id: fold(id),
    description: fold(description),
    categories: (Array.isArray(categories) ? categories : []).map(fold),
  };
}

// The best (tier, score) of one query term against one entry, or null.
function scoreTerm(term, e) {
  const name = scoreText(term, e.name);
  if (name !== null) return { tier: TIER_NAME, score: name };
  let best = null;
  const take = (s) => {
    if (s !== null && (best === null || s > best)) best = s;
  };
  take(scoreText(term, e.id));
  for (const c of e.categories) take(scoreProse(term, c));
  take(scoreProse(term, e.description));
  return best === null ? null : { tier: TIER_OTHER, score: best };
}

// The query's terms: split on spaces, folded. Each term must match (fzf's AND), so
// "code ext" narrows rather than widens.
export function terms(query) {
  return fold(query).split(/\s+/).filter(Boolean);
}

// How one entry answers the query: null when a term misses, else its tier (the worst of its
// terms — a row is a name match only when every term found its name) and its summed score.
export function matchEntry(query, e) {
  const ts = terms(query);
  if (ts.length === 0) return null;
  let tier = TIER_NAME;
  let score = 0;
  for (const t of ts) {
    const r = scoreTerm(t, e);
    if (r === null) return null;
    tier = Math.max(tier, r.tier);
    score += r.score;
  }
  return { tier, score };
}

// The matches, best first: the name tier first, then by score, then the shorter name (fzf's
// tie-break: the tighter text is the likelier target), then the order given — stable, so two
// equal rows keep the catalogue's order. Returns `[{ key, tier, score }]`, matches only.
// `entries` is `[{ key, entry }]`; the key is the caller's (app.js uses the row index).
export function rank(query, entries) {
  const out = [];
  entries.forEach(({ key, entry }, pos) => {
    const r = matchEntry(query, entry);
    if (r) out.push({ key, pos, len: entry.name.length, ...r });
  });
  out.sort((a, b) => a.tier - b.tier || b.score - a.score || a.len - b.len || a.pos - b.pos);
  return out.map(({ key, tier, score }) => ({ key, tier, score }));
}

// The whole view a query asks for, still DOM-free: which rows show, in which order, and
// which categories open. `rows` is `[{ key, cat, entry }]` in the catalogue's order.
//
// - `matched`: the keys that match, best first.
// - `cats`: every category, those holding a match first (ordered by their best row), the
//   others after them in the catalogue's order; each `{ cat, open }`, open when it holds a
//   match. A category without one FOLDS rather than vanishes: its header stays, so the
//   reader still sees what the catalogue holds.
//
// An empty query answers null: there is no find, the catalogue's own view stands.
export function findView(query, rows) {
  if (terms(query).length === 0) return null;
  const ranked = rank(query, rows.map(({ key, entry }) => ({ key, entry })));
  const catOf = new Map(rows.map((r) => [r.key, r.cat]));
  const hit = [];
  for (const { key } of ranked) {
    const c = catOf.get(key);
    if (!hit.includes(c)) hit.push(c);
  }
  const all = [];
  for (const r of rows) if (!all.includes(r.cat)) all.push(r.cat);
  const cats = [
    ...hit.map((cat) => ({ cat, open: true })),
    ...all.filter((c) => !hit.includes(c)).map((cat) => ({ cat, open: false })),
  ];
  return { matched: ranked.map((r) => r.key), cats };
}
