# RESUME — issue #7451 work in progress

**These files are scratch state, not deliverables. Delete the whole `.wip-7451/`
directory in the final commit before opening the PR.** They live here only because
`.claude/worktrees/` and the git-common-dir run records are gitignored, and this
container is ephemeral.

## Status: plan-review loop, 11 rounds done, ROUND-12 REVIEW NOT YET RUN

`plan-7451.md` (3146 lines) is a **coherent, complete artifact** — round 11 finished
cleanly and returned a full delta report. This is unlike the previous stop, where the
file was a half-applied edit; that condition no longer applies.

**The one thing outstanding: round 11's changes have not been independently reviewed.**
Every prior round was reviewed by a fresh agent, and rounds 7, 8, 9 and 10 each caught a
doc-comment that would have shipped false. Round 11 is the only round whose output has
not been through that check, and it was a substantial round (2851 → 3146 lines, two new
sub-steps, a new §U1.4(d), and two self-swept findings). **Do not skip straight to
implementation.** Run round 12 first.

## Pipeline state

- Skill: `/engine-implementer`, run id `7451`.
- `BASE_SHA` = `515175f7151a810be1346f5d30e45535a2ae2abe` (== `upstream/main` when fixed).
- Branch `claude/phase-rs-parsing-issues-ofyhm3`, pushed to `origin` (fork
  `alicewonderland-dev/phase`). `upstream` = `phase-rs/phase` (git read only).
- Step 1a phase-fit: **SINGLE-PHASE**, re-adjudicated four times, independently confirmed
  by five reviewers. T1 fires (3 units), T2 does not (7 scope paths, threshold 13).
- Step 2 plan-review rounds completed: **11**. Findings per round:
  14 → 11 → 9 → 9 → 2 → 2 → 3 → 1 → 1. Zero blocking in rounds 5, 6, 8, 9; one blocking
  in round 10 (the four-site census), none since.
- Surgical-fix mode was entered twice (rounds 7 and 10) and exited twice, both times
  because a finding required deciding something or adding structure. All measurements
  and both exits are recorded in `surgical-mode-switch.txt`.
- T4 has never fired: blocking findings span 2 axis layers (parser, tests) and the
  trigger needs 3.
- Steps 3–7 (implement / checkpoint / verify / review-impl / accept): **not started.**
  **No production code has been written.** Working tree is clean apart from `.wip-7451/`.

## The issue

Upstream `phase-rs/phase#7451` — *"Parser: multi-subtype filter lists collapse to the
last subtype (\"Birds, Frogs, Otters, and Rats you control\" → Rats only)"*.
`area:parser`, `bug`, `classifier:supported-aspect-defect`,
`mechanic:continuous-effects`, `priority:p2-wrong-game-result`, `source:internal-triage`,
`status:confirmed`. Re-checked at the last stop: still open, still no linked PR.

Selected because `classifier:supported-aspect-defect` is defined upstream as *"Card
claims to support the clause but AST is unfaithful"* — exactly "wrong behavior but claims
full parse support" — and it was the widest-reach such issue with no PR.

## Substantive findings so far (do not re-derive — each independently confirmed)

- **The issue's named seam is wrong.** `parse_type_phrase` / `merge_or_filters` /
  `oracle_nom/filter.rs` are healthy; the same Oracle text parses correctly when it is
  not a trigger effect. The real defect is the trigger condition/effect boundary scanner
  — `is_new_sentence_not_type_continuation`'s one-item window in `oracle_trigger.rs`.
- **Two of the four consumers the issue demands are already correct** (protection lists,
  quantity counts) and get regression pins, not fixes. The third ("except for"
  exclusions) is broken by two independent causes.
- **Core-type lists collapse identically** (`artifacts, creatures, and lands` → `Land`),
  so a subtype-scoped fix would build for the card, not the class.
- **5 printed cards** move from wrong-and-silent to correct: Valley Floodcaller, Whelming
  Wave, Slinn Voda, Cyclone Summoner, The Argent Etchings. Plus 33 condition-side cards
  protected from regression.
- Three near-misses the design must not regress, all correct today: **The Thanos-Copter**,
  **Spellbinding Soprano**, **Immolation Shaman** (the last found regressing under an
  earlier design and it drove the current window bound).

## Round 11's changes — the part needing review

1. **The blocking round-10 finding:** the event-head census figure is written at **four**
   sites in `oracle_trigger.rs` (`:9126`, `:9182`, `:9395`, `:9523`), not two. §U1.4
   updated two and would have shipped two false doc-comments — `:9395` is the bad one, in
   `parse_state_change_event_start`'s doc, cross-referencing the very census §U1.4(a)
   rewrites, and already wrong today by exactly the `:8971` omission §U1.4 exists to
   repair. Round 11 added §U1.4(b2) and §U1.4(b3).
2. **The invariant it chose (its judgement call):** after U1, exactly two lines carry the
   census FIGURE (`:9126`, `:9523`); every other in-file reference is figure-free and
   names `parse_event_head_start` as where the number lives. Enforced by three greps in
   §U1.4(c) — G1 (durable, number-agnostic, 2 → 2), G2 (pins the pair 4/11, 0 → 2),
   G3 (sweeps the superseded figure, **4 → 0**, its 4-at-BASE being the positive
   reach-guard). It validated these against a **simulated post-U1 file**
   (`<scratchpad>/sim_u1.py` → `oracle_trigger.SIM.rs`).
3. **Its own exhaustive sweep found two more**, both fixed, both comment-only and inside
   existing scope paths:
   - **S1 / new §U1.4(d):** three hard-coded `oracle_trigger.rs:NNNN` citations are
     **already wrong at BASE_SHA** and every U1 insertion sits above every target.
     ORCHESTRATOR-VERIFIED: `:9139` cites `try_parse_event` at `:10225` but the fn is at
     **`:11868`**; the tag is at **`:12366`**, not `:10622`; `oracle_trigger_tests.rs:29697`
     cites `:17850` (true `:19856`). Repaired by reformulation to symbol names, not by
     refreshing numbers.
   - **S2:** §U3.1's shipping doc-comment said "the two lines the ONE existing call site
     already runs", but the commit shipping that sentence is the commit that gives the
     parser a second call site. Replaced with a symbol reference.
   It reports sweeping and finding clean: `oracle_static/evasion.rs:342`–`:349`,
   `oracle_target.rs:3269`–`:3275`, the Mageta/Flame Sweep test docs,
   `oracle_trigger.rs:9014`–`:9034` (a third narrow/wide enumeration that stays true),
   `:8965`–`:8970`, `:9027`, `:9222`, the "FOUR hand-maintained lexicons" count and both
   consolidation triggers. One pre-existing imprecision reported and deliberately NOT
   edited: `oracle_trigger.rs:1106` / `oracle_trigger_tests.rs:25302` describe
   `cond_lower` as "everything before the FIRST comma", already loose at BASE_SHA and
   neither created nor worsened by U1.

**What round 12 should scrutinise:** all of the above is self-reported by the agent that
made the changes. Verify the invariant is actually enforced by the three greps as
claimed, that the simulated-file methodology is sound, that S1's reformulations are
correct, and — the recurring failure mode — run the "one fix, one stale twin" sweep
again, since round 11 rewrote §U1.4 wholesale for the second time in four rounds.

## Environment notes (all warm on disk at the time of stopping — verify on resume)

- **Tilt is NOT running.** The CLAUDE.md "use Tilt logs, never cargo" rule inverts here;
  run cargo directly per the documented fallback
  (`tilt get uiresource clippy >/dev/null 2>&1`; non-zero = down).
- A full `phase-engine` build peaks near **6.6 GB RSS**, ~10 min on this 16 GB box. Use
  `-j 2`. Two concurrent engine builds OOM-kill each other — that already happened once.
- `client/public/card-data.json` (98 MB, generated at BASE_SHA) — gitignored, regenerate
  with `MTGJSON_SKIP_REFRESH=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 ./scripts/gen-card-data.sh`
  (~40 min) if the container was reclaimed.
- Scratchpad root `/tmp/claude-0/-home-user-phase/77e33f0e-b340-5f16-bac7-81cf00cddb8f/scratchpad`:
  `card-data.BASE.json`, `ast-digest.BASE.jsonl`, `harvest.r6.jq`, `trigger-lines.r6.tsv`,
  plan backups `.r4/.r5/.r7.bak`, `sim_u1.py`, and a **warm 5.5 GB `probe-target`** with
  the `probe7451` crate. **All lost if the container is reclaimed** — regenerate before
  trusting any before/after comparison.
- The card-data export **dirties tracked files** as a side effect (observed:
  `crates/engine/data/mtgjson-vintage`, a date stamp; CLAUDE.md documents the same for
  `oracle-subtypes.json` and `known-tokens.toml`). Restore them; never `git add -A`.
- `docs/MagicCompRules.txt` is gitignored; `./scripts/fetch-comp-rules.sh` (~1 min)
  before any CR verification.

## GitHub access constraint (still unresolved — affects the PR step)

This session's GitHub **API** access is scoped to the fork `alicewonderland-dev/phase`.
`phase-rs/phase` is readable over git and over the web (WebFetch against the HTML pages
works and is how the issue was found and re-checked), but `add_repo` refuses to attach it
("cross-tier adds are not supported in v1"), and `api.github.com` is blocked via both
curl and WebFetch. So:

- `git push` to the fork works. Pushing to `phase-rs/phase` does not.
- Opening a PR against `phase-rs/phase` **has not been attempted** and is expected to fail
  through the GitHub MCP tools, which refuse out-of-scope repositories.
- If it does fail, the options are: open the PR from a fresh session started with
  `phase-rs/phase` as its initial source, or push the branch to the fork and hand the
  user a compare link. Do not silently open the PR somewhere else.

## Next actions on resume

1. **Run round 12** — a fresh Opus reviewer via `/review-engine-plan` against
   `plan-7451.md`, briefed on round 11's changes above and told a clean verdict is the
   expected outcome if the plan is genuinely ready.
2. Continue the loop until a round returns zero gaps. It is unbounded by design; do not
   stop at "one more round and ship". Findings are converging hard (last four rounds:
   3, 1, 1, and round 11 not yet reviewed).
3. Then Steps 3–7: `engine-implementation-executor` (Sonnet) against the frozen 7-path
   scope → orchestrator-owned checkpoint commit → verification at the committed candidate
   (including the plan's blocking step 5.5 boundary census and step 6 corpus AST diff)
   → `/review-impl` (Opus) → acceptance.
4. **Delete `.wip-7451/` in the final commit** before opening the PR.
