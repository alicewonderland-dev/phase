# HANDOFF — issue #7451, plan complete, implementation not started

Read this first, then `plan-7451.md` in this directory.

**You are picking up a `/engine-implementer` run at Step 3.** Eleven plan-review
rounds are done. No production code has been written. Your job is to run the
outstanding review round, then implement, verify, and open the PR — under
`docs/AI-CONTRIBUTOR.md`, which the cloud session that produced this could not
complete because it had no GitHub API access to `phase-rs/phase`.

---

## 0. What you must follow

**`docs/AI-CONTRIBUTOR.md` is binding on this run**, plus `CLAUDE.md` and the
skills under `.claude/skills/`. The sections that will actually bite:

| § | Requirement | Note for this run |
|---|---|---|
| §0 | Frontier-tier model; no pausing for human input; tool access | **Declare YOUR model, not the one that wrote the plan.** The plan was produced by `claude-opus-5`; that has no bearing on your `Model:` line. Misreporting is the one thing that escalates to an account-level problem. |
| §0.1.1 | Frontier tier or abort | If you are below the floor, abort and say so — do not open the PR. |
| §0.1.2 Gate A | `./scripts/check-parser-combinators.sh` after the final commit + final review | Paste raw output under `## Gate A`. `head=` must equal the PR head. |
| §0.1.2 Gate B | ≥2 analogous implementations cited under `## Anchored on` | The plan already names them — see §5 below. |
| §0.1.4 | `Tier: Frontier` on its own line | |
| §2.1 | Sync fork with upstream every run | |
| §3.4 | Confirm no open PR covers this issue or its mechanic | **Do this before writing code.** The cloud session could not (no API access). |
| §4 | Branch cut from `upstream/main`, not fork `main` | Critical here — see §2 below. |
| §5 | Mandatory final read-only `/review-impl` against the committed head, then Gate A | Record `Final review-impl PASS head=<sha>`. Any later commit invalidates both. |
| §6 | Developer-track verification | Plus the plan's own blocking steps 5.5 and 6 — see §6 below. |
| §7 | Scope gate, then PR from `.github/PULL_REQUEST_TEMPLATE.md` | `Closes #7451`. |

---

## 1. The issue

Upstream **`phase-rs/phase#7451`** — *"Parser: multi-subtype filter lists collapse
to the last subtype (\"Birds, Frogs, Otters, and Rats you control\" → Rats only)"*.
`area:parser`, `bug`, `classifier:supported-aspect-defect`,
`mechanic:continuous-effects`, `priority:p2-wrong-game-result`, `status:confirmed`.
Open, unassigned, no linked PR as of the last check.

**The issue's own diagnosis is wrong, and the plan documents why.** It names
`oracle_target.rs::parse_type_phrase` / `oracle_util.rs::merge_or_filters` /
`oracle_nom/filter.rs` as the seam. Those are healthy — the same Oracle text parses
correctly when it is *not* a trigger effect. The real defect is the trigger
condition/effect boundary scanner (`is_new_sentence_not_type_continuation`'s
one-item window in `oracle_trigger.rs`). Two of the four consumers the issue
demands are already correct and need regression pins, not fixes. Seven independent
reviewers confirmed this. **Do not "fix" the seam the issue names.**

---

## 2. Repo state — read before branching

The branch `claude/phase-rs-parsing-issues-ofyhm3` on the fork
`alicewonderland-dev/phase` carries **two WIP commits that must never reach the
PR** (`40c626b`, `50f1ef2`). They exist only because `.claude/worktrees/` is
gitignored and the cloud container was ephemeral. They contain this directory and
nothing else — no production code.

**Recommended: start clean.**

```bash
git fetch upstream main
git fetch origin claude/phase-rs-parsing-issues-ofyhm3
mkdir -p /tmp/7451 && git show origin/claude/phase-rs-parsing-issues-ofyhm3:.wip-7451/plan-7451.md > /tmp/7451/plan-7451.md
git checkout -B fix/7451-oxford-type-list-boundary upstream/main
```

Then work from `/tmp/7451/plan-7451.md`. Whatever you do, **`git diff --stat
upstream/main...HEAD` at §7 must show only the plan's seven scope paths** — no
`.wip-7451/`.

`BASE_SHA` the plan was written against: `515175f7151a810be1346f5d30e45535a2ae2abe`.
If `upstream/main` has moved, re-verify the plan's line anchors before editing
(§3 below) — the plan is anchor-dense and rebasing will shift them.

---

## 3. Outstanding: round 12 of the plan-review loop

**Run this before implementing.** Round 11 was substantial (2851 → 3146 lines: two
new sub-steps, a new §U1.4(d), and two findings it discovered in its own sweep) and
is **the only round whose output has not been independently reviewed**. Rounds 7,
8, 9 and 10 each caught a doc-comment that would have shipped false, so the check
is not ceremonial.

Spawn a fresh agent (Opus, no prior context) invoking `/review-engine-plan` against
the plan. Tell it a clean verdict is the expected outcome if the plan is genuinely
ready, and that manufacturing findings to look thorough is a failure — six of the
last seven rounds returned zero blocking findings and the loop is converging
(findings by round: 14 → 11 → 9 → 9 → 2 → 2 → 3 → 1 → 1).

Point it at, specifically:

1. **Round 11's invariant and its enforcement.** After U1, exactly two lines in
   `oracle_trigger.rs` carry the census *figure* (`:9126`, `:9523`); every other
   in-file reference is figure-free and names `parse_event_head_start` as where the
   number lives. Enforced by three greps in §U1.4(c): G1 (durable, number-agnostic,
   2 → 2), G2 (pins the pair 4/11, 0 → 2), G3 (sweeps the superseded figure,
   **4 → 0**, its 4-at-BASE being the positive reach-guard). Round 11 validated
   these against a simulated post-U1 file — check that methodology is sound.
2. **§U1.4(d) / finding S1.** Three hard-coded `oracle_trigger.rs:NNNN` citations
   are *already wrong at BASE_SHA* and every U1 insertion sits above every target.
   Verified independently: `:9139` cites `try_parse_event` at `:10225` but the fn
   is at **`:11868`**; the tag is at **`:12366`**, not `:10622`;
   `oracle_trigger_tests.rs:29697` cites `:17850` (true `:19856`). Round 11 repaired
   by reformulation to symbol names, not by refreshing numbers — confirm that.
3. **Finding S2.** §U3.1's doc-comment said "the two lines the ONE existing call
   site already runs", but the commit shipping that sentence is the commit that
   gives the parser a second call site.
4. **The recurring failure mode**, one more time: "one fix, one stale twin" — a
   repair landing at one site while the codebase carries the claim at several. It
   has been the finding in **seven of eleven rounds**, and round 11 rewrote §U1.4
   wholesale for the second time in four rounds.

If it returns findings, loop (fresh planner → fresh reviewer) until clean. The loop
is unbounded by design; do not stop at "one more round and ship".

---

## 4. What the change is

Three units, one dependency edge (U2 → U3). Full detail in the plan; this is
orientation only.

- **U1** — `oracle_trigger.rs`. `is_new_sentence_not_type_continuation`'s window is
  one list item, so the boundary scanner walks past the commas of the effect
  clause's own Oxford type list and lands on the last one. Valley Floodcaller's
  `PumpAll` ends up targeting `[Subtype("Rat")]` with `parse_warnings` empty, and
  the swallowed items pollute the trigger's `valid_card`. The fix is a **monotone**
  two-pass predicate: `new(x) = old(x) ∨ (widened-window scan with an
  event-head-priority classifier)`, with the scan *stopped* at a restrictive
  postmodifier while every parser it consults still sees the unbounded tail.
  Core-type lists collapse identically (`artifacts, creatures, and lands` → `Land`),
  so this is not subtype-specific.
- **U2** — `oracle_target.rs`. `parse_except_for_type_list_suffix` declines the whole
  clause when an item resolves to `Non(Subtype(_))`, and singularises with
  `trim_end_matches('s')` (mangling `Octopuses` → `"Octopuse"`). Gated on the
  subtype vocabulary instead.
- **U3** — `oracle_effect/imperative.rs` + `oracle_target.rs`. For
  `return … to their owners' hands except for …`, the exclusion lands in
  `dest_remainder`, which only the battlefield branch reads, so it is dropped.

**Five printed cards move from wrong-and-silent to correct:** Valley Floodcaller,
Whelming Wave, Slinn Voda, Cyclone Summoner, The Argent Etchings. Plus 33
condition-side cards protected from regression.

**Three near-misses the design must not regress, all correct today:** The
Thanos-Copter, Spellbinding Soprano, Immolation Shaman. The last was found
regressing under an earlier design and drove the current window bound — its pin is
load-bearing, do not weaken it.

---

## 5. Verified — do not re-derive

Each of these was measured and independently confirmed, most of them more than
once. Re-deriving them is wasted effort; *contradicting* one is a signal something
has moved and you should stop and check.

- **Sizing: 3 units / 7 scope paths / SINGLE-PHASE.** Re-adjudicated four times,
  walked independently by five reviewers. `game/keywords.rs`, `game/scenario.rs`,
  `oracle_static/mod.rs`, `oracle_effect/tests.rs` are all genuinely designed out;
  `client/public/card-data.json` is gitignored.
- **The event-head census unit**: today 10 consultation sites = 7 NARROW
  (`:8939, :9051, :9073, :9080, :9844, :9850, :9851`) + 3 WIDE
  (`:8971, :9131, :9705`); U1 makes it 11 = 7 + 4.
- **`PREDICATE_VERBS` has exactly 47 entries**; `control`, `enter`, `enters` absent
  (the Questcaller and April O'Neil pins depend on this); `scry`, `put`, `mill`
  present. `parse_event_phrase` is a bare `tag` with no boundary peek — that is why
  the structural repair is load-bearing.
- **All 15 CR rows verified verbatim** against `docs/MagicCompRules.txt`, with
  CR 603.2 correctly *excluded* and CR 608.2c honestly demoted to an analogy for
  U1 while staying a direct citation for U3. Two earlier rounds had to retract a
  citation that dressed a parser heuristic as a rule — do not add one.
- **Gate B anchors are already in the plan** (§Building Blocks and §Step 2, which
  traces the #5324 Sram Oxford-comma scan as the analogous feature). Use those for
  `## Anchored on`; verify the `file:line` refs still resolve before citing them.
- The `except for` corpus census (17 distinct clauses), the §5.5 harvest
  (16862 lines = 15731 bare + 1131 ability-word, 14933 cards), `parse_warnings` on
  839/35800 entries — all reproduce.

---

## 6. Verification the plan makes blocking

Beyond §6 of AI-CONTRIBUTOR (`cargo fmt`, clippy, `test -p phase-engine`,
`gen-card-data`, `cargo coverage`, `cargo semantic-audit`):

- **Step 5.5 — pre-flight boundary census.** A temporary `#[ignore]` test in
  `oracle_trigger_tests.rs` calling `find_effect_boundary` over the harvested corpus
  trigger lines, run at BASE and post-U1. It **must not survive the commit**. Its
  point is catching a second Immolation Shaman *before* the expensive regeneration.
- **Step 6 — corpus AST digest diff**, mandatory and blocking. Allowlists are tied
  to commit shape: after commit 1 (U1 only) the restricted diff must name exactly
  `valley floodcaller`; after commits 2–3, exactly `valley floodcaller`,
  `slinn voda, the rising deep`, `cyclone summoner`; the unrestricted diff those
  three plus `whelming wave` and `the argent etchings` — **five**. A *missing* entry
  is as much a stop as an extra one.
- **Coverage must be flat.** No card should flip supported↔unsupported; these five
  were already "supported" and merely wrong. If the headline number moves,
  investigate before shipping.

Commit shape the plan specifies: three commits in dependency order, each
independently green (U1 + its tests; U2 + its tests; U3 + its tests).

---

## 7. Environment caveats

The cloud container had no Tilt, so the plan's verification prose assumes the
documented cargo fallback. **If Tilt is running locally, prefer it** per CLAUDE.md
and AI-CONTRIBUTOR §6 — the plan says so too.

- A full `phase-engine` build peaked at **~6.6 GB RSS** / ~10 min on a 16 GB box.
  Two concurrent engine builds OOM-killed each other once. Use `-j 2` if memory-tight.
- `./scripts/gen-card-data.sh` takes ~40 min from cold (MTGJSON download + build +
  export). It **dirties tracked files** as a side effect — observed
  `crates/engine/data/mtgjson-vintage`; CLAUDE.md documents the same for
  `oracle-subtypes.json` and `known-tokens.toml`. Restore them; commit by explicit
  pathspec, never `git add -A`.
- `docs/MagicCompRules.txt` is gitignored — run `./scripts/fetch-comp-rules.sh`
  before any CR verification.
- The cloud scratchpad (baseline `card-data.BASE.json`, AST digest, harvest
  artifacts, the simulated post-U1 file, a warm probe crate) is **gone**. Regenerate
  the BASE snapshot before trusting any before/after comparison.

---

## 8. Why the plan is this long

3146 lines for a parser fix looks disproportionate. It is the residue of eleven
review rounds, and each of the first four caught something that would have shipped
a regression:

- a design that would have **lost the entire effect** on two cards that work today;
- a printed-card regression (**Immolation Shaman**), found only by simulating the
  proposed predicate over the whole corpus;
- an **unsound containment proof** whose conclusion the verification allowlist was
  derived from;
- a pre-flight census that **silently skipped ~8% of its own population** while its
  reach-guard still passed.

Treat the plan as adjudicated. If you disagree with a design decision, the bar is
evidence, not preference — and if you find something genuinely wrong, that is worth
knowing, because it means round 12 should have caught it.
