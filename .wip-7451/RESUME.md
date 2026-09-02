# RESUME — issue #7451 work in progress

**These files are scratch state, not deliverables. Delete the whole `.wip-7451/`
directory in the final commit before opening the PR.** They live here only because
`.claude/worktrees/` and the git-common-dir run records are gitignored, and this
container is ephemeral.

## Status: plan-review loop, round 4 INTERRUPTED MID-EDIT

`plan-7451.md` in this directory is **NOT a consistent artifact**. It is a partial
round-3 → round-4 revision: the round-4 planner was stopped by the operator (usage
limit) while it was working, with its last progress note being *"Now the U1.2
doc-comment and U1.3 code block."* Some round-3 review findings are applied, some
are not, and the U1.2/U1.3 sections may be internally inconsistent.

**Do not hand this file to an implementer.** On resume, re-run round 4 from scratch:
dispatch a fresh planner (Opus) with this file plus the complete round-3 findings
reproduced below, then a fresh reviewer (Opus) for round 4 of `/review-engine-plan`.

## Pipeline state

- Skill: `/engine-implementer`, run id `7451`.
- `BASE_SHA` = `515175f7151a810be1346f5d30e45535a2ae2abe` (== `upstream/main` at the time).
- Branch: `claude/phase-rs-parsing-issues-ofyhm3`, pushed to `origin` (the fork
  `alicewonderland-dev/phase`). `upstream` remote = `phase-rs/phase` (git read only).
- Step 1a phase-fit: **SINGLE-PHASE**. T1 fires (3 units), T2 does not (7 scope
  paths, threshold 13). Re-adjudicated after rounds 2 and 3; verdict unchanged and
  independently confirmed by the round-3 reviewer. Record in `phase-fit.txt`.
- Step 2 plan-review rounds completed: **3**. Findings per round: 14 → 11 → 9
  (converging). T4 has never fired: blocking findings span 2 axis layers
  (parser, tests), and the trigger needs ≥3.
- Steps 3–7 (implement / checkpoint / verify / review-impl / accept): **not started.**
  No production code has been written. The working tree is clean apart from this
  directory.

## The issue

Upstream `phase-rs/phase#7451` — *"Parser: multi-subtype filter lists collapse to the
last subtype (\"Birds, Frogs, Otters, and Rats you control\" → Rats only)"*.
Labels: `area:parser`, `bug`, `classifier:supported-aspect-defect`,
`mechanic:continuous-effects`, `priority:p2-wrong-game-result`,
`source:internal-triage`, `status:confirmed`. No linked PR.

Selected because `classifier:supported-aspect-defect` is defined upstream as *"Card
claims to support the clause but AST is unfaithful"* — exactly "wrong behavior but
claims full parse support" — and this was the widest-reach such issue with no PR.

The issue names four consumers a complete fix must satisfy: (1) anthem-style mass
pumps, (2) protection lists, (3) "except for" mass-bounce exclusions, (4) quantity
counts.

## Key findings so far (independently corroborated, worth not re-deriving)

- **The issue's named seam is wrong.** `parse_type_phrase` /`merge_or_filters` /
  `oracle_nom/filter.rs` are healthy. The real defect for consumer 1 is the trigger
  condition/effect boundary scanner: `is_new_sentence_not_type_continuation`'s
  one-item window (`text.split(", ").next()`) in
  `crates/engine/src/parser/oracle_trigger.rs`. The same Oracle text parses
  correctly when it is *not* a trigger effect.
- **Consumers 2 and 4 are already correct** (measured) and get regression pins, not
  fixes. Consumer 3 is broken by two independent causes in
  `parse_except_for_type_list_suffix` and in the `dest_remainder` handling of the
  mass-return dispatcher.
- **Core-type lists collapse identically** (`artifacts, creatures, and lands` → `Land`),
  so a subtype-scoped fix would be building for the card, not the class.
- Confirmed at `BASE_SHA` from generated card data: Valley Floodcaller's `PumpAll`
  target is `type_filters:[{"Subtype":"Rat"}]` with `parse_warnings: null` and a
  `valid_card` `Or` polluted with Bird/Frog/Otter legs; Whelming Wave's `BounceAll`
  target is `[Creature]` with the exclusion dropped.
- **5 printed cards** move from wrong-and-silent to correct (Valley Floodcaller,
  Whelming Wave, Slinn Voda, Cyclone Summoner, The Argent Etchings), plus 33
  condition-side cards protected from regression.
- Two near-misses the design must NOT regress, both correct today: **The
  Thanos-Copter** and **Spellbinding Soprano**. A third, **Immolation Shaman**, was
  found regressing under the round-2 design and drove the round-3 window bound.

## Round-3 review findings — feed these verbatim to the round-4 planner

### BLOCKING

**G1 [DESIGN — the crux] §U1.1's "Containment lemma" is UNSOUND: a shrinking window
can flip a verdict TOWARD `true`.** The lemma assumes the classifier's verdict at a
word position depends only on the position. It does not: the closure calls
`parse_event_head_start(tail)` where `tail` is a slice OF THE BOUNDED WINDOW.
`parse_event_phrase` is a bare `tag(phrase)` with NO boundary peek
(`oracle_trigger.rs:9271-9274`), so every phrase-form event head needs its trailing
space INSIDE the window: `"deals "`, `"deal "`, `"draws "`, `"draw "`, `"casts "`,
`"cast "`, `"creates "`, `"create "`, `"sacrifices "`, `"discard(s) "`, `"attack "`,
`"block "`, `"explore "`, `"is put into"` (`:9280-9333`). If the postmodifier bound
truncates immediately after such a verb, the Event match FAILS and the same word is
then read as a `PREDICATE_VERBS` hit (`deal`, `cast`, `create`, `sacrifice`,
`discard`, `draw`, `attack`, `block`, `explore` are all in the 47-entry table) —
i.e. `pass2_r3(x) = true` where `pass2_r2(x) = false`. The lexical shapes exist in
the corpus (trigger descriptions contain `deals that` ×74, `cast that` ×93, `draw
that` ×53, `create that` ×36, `discards that` ×18, `sacrifice that` ×13, `casts
that` ×5). No printed card also satisfies the surrounding geometry (comma +
`starts_with_type_word` + pass-1 false), so this is a PROOF DEFECT, not a
demonstrated regression — but it is exactly the proof the plan uses to bound H2 to
one card, and it is what Verification-step-6's one-entry allowlist is derived from.
REQUIRED — pick one and justify:
 (a) STRUCTURAL REPAIR (reviewer's recommendation): evaluate `parse_event_head_start`
 on the UNBOUNDED sentence tail while bounding only where the SCAN STOPS. Drop the
 `.to_lowercase()` (the doc already establishes `text` is lowercase —
 `find_effect_boundary(tp.lower)`), keep `let window: &str = type_list_clause_window(text)`,
 and drive the scan by OFFSET into `text` so the Event parser always sees the full
 tail. Immolation Shaman is still saved because the SCAN stops at `that `. This
 restores the lemma exactly.
 (b) Delete the lemma's "therefore the r3 change set is exactly {valley floodcaller}"
 conclusion, state H2 as discharged ONLY by the post-fix corpus diff, and rewrite
 step 6 accordingly.

**G2 [SPOT] §Verification steps 6 — the blocking allowlist is WRONG for the tree it
runs against.** Measured: `.["slinn voda, the rising deep"].triggers[0].description`
= `"When ~ enters, if it was kicked, return all creatures to their owners' hands
except for Merfolk, Krakens, …"` and `.["cyclone summoner"].triggers[0].description`
= `"When ~ enters, if you cast it from your hand, return all permanents … except for
Giants, Wizards, and lands."` — BOTH contain `", "`, so both ARE selected by the
digest's `select(any(.value.triggers[]?; …contains(", ")))`, and both change AST
under U2+U3. Run after all three commits, the restricted diff names three entries and
the plan orders the implementer to STOP. (Whelming Wave has zero triggers; The Argent
Etchings' trigger descriptions are `"Chapter 1/2/3"` — neither appears in the
restricted diff; they surface only in the unrestricted one.) Symmetrically the
UNRESTRICTED diff's stated expectation ("exactly whelming wave, slinn voda, cyclone
summoner and the argent etchings and nothing else") OMITS `valley floodcaller`, which
U1 changes.
EXACT REPLACEMENT: tie step 6 to the commit shape — "**After commit 1 (U1 only)** the
restricted-diff allowlist is exactly `valley floodcaller`. **After commits 2–3** it is
exactly `valley floodcaller`, `slinn voda, the rising deep`, `cyclone summoner`; and
the unrestricted diff's allowlist is those three plus `whelming wave` and `the argent
etchings` — FIVE entries, not four."

**G3 [SPOT] §Verification Matrix, V1u-imm, Harsh Mentor row — the pinned expectation
is measurably wrong and contradicts the plan's own trace table.** The plan requires
the `find_effect_boundary` suffix to start `", or land on the battlefield"` (the
card's SECOND comma). Measured: `.["harsh mentor"].triggers[0].valid_card =
Typed{[AnyOf["Artifact","Creature","Land"]], properties:[InZone Battlefield]}`,
`effect = DealDamage{2, TriggeringPlayer}` — the condition retains ALL THREE type legs
and `on the battlefield`, so today's boundary is at the THIRD comma. (At comma 2 the
narrow window is `"land on the battlefield"`, whose words `on/the/battlefield` are
neither predicates nor negated auxiliaries, so `continues_player_action_list` returns
true.) The plan's own §U1.1 trace row and its regression-risk row both say "unchanged"
i.e. comma 3 — only the TEST row is wrong.
EXACT REPLACEMENT: "…Harsh Mentor, `"…of an artifact, creature, or land on the
battlefield, if it isn't a mana ability, …"` — suffix must start **`", if it isn't"`**
(the card's THIRD comma, exactly where it is today; `valid_card` keeps
`AnyOf[Artifact, Creature, Land]` and `InZone Battlefield`)."

**G4 [DESIGN] §U1.1 "Discharging H2 with corpus evidence" — the load-bearing
measurement has NO regeneration command and is not reproducible.** Every other census
in the plan is stated with its exact command, and the round-3 reviewer re-ran C1 (6),
C2 (1), C3 (35 entries / 33 cards / 1 Unknown), C4 (3), C5 (4) and the 15307 count —
ALL MATCH. The ONLY figure with no command is the one the whole safety argument rests
on: "the round-2 reviewer simulated `old(x) or pass2(x)` over every trigger line…
Result: exactly two cards change." A reviewer-attributed, uncommanded count in a plan
that explicitly says "no claim is [PROBED] for the post-fix tree" is not a discharge.
REQUIRED: either include the RUNNABLE simulation (script + invocation + positive
control) so it can be re-run against the round-4 predicate, or downgrade the claim to
"unverified prior measurement" and make Verification step 6 the SOLE discharge of H2,
with the allowlist stated as an expectation to investigate rather than a proven set.

**G5 [SPOT] §Verification Matrix "Parser-accepted-but-semantics-deferred: None" is
FALSE, and the U3 doc-comment repeats the error.** The section asserts U2/U3's
fail-closed declines keep coverage "honestly red". Measured: `mageta the lion →
parse_warnings = null` and `flame sweep → parse_warnings = null` — both report
SUPPORTED while silently dropping their exclusion (Mageta destroys itself; Flame Sweep
damages its controller's fliers). `swallow_check.rs` has no `except for` detector, as
the plan's own Evidence section states. The decline path is SILENT, not red — the same
defect class as #7451.
EXACT REPLACEMENT: "**Parser-accepted-but-semantics-deferred: the `except for` decline
paths.** A declined clause (named exception, mixed name list, non-`Typed` population,
non-hand destination) is dropped **silently** — `parse_warnings` stays empty and the
card still reports supported (measured at BASE_SHA: `mageta the lion`, `flame sweep`).
This is unchanged from today and out of scope for #7451; making it honest needs a
`swallow_check` `except for` detector, filed separately."

### MATERIAL GAPS

**G6 [SPOT] CR scoping — the "event heads win ties" rule is a heuristic wearing a CR
citation.** All 13 CR line numbers are verbatim-correct (re-grepped: 2559, 2576, 1408,
1416, 1435, 1441, 2797, 1954, 2908, 610, 4051, 4067, 3266, plus the removed 2565). But
§U1.3's doc-comment says "EVENT HEADS WIN TIES — CR 603.1 + CR 603.2e … in a CR 603.1
sentence the first classifying token after a type list belongs to the trigger event if
the event lexicon claims it." CR 603.1 states only the sentence template; it does NOT
state a first-token tie-break. Same shape as the round-1 CR 603.2 retraction.
REPLACEMENT: cite CR 603.1 for "the template places the trigger condition/event BEFORE
the effect", then state the tie-break as an engineering heuristic derived from that
ordering and validated by the fixture set (Theorem 3a), not as a rule the CR states.
Two smaller nits in the same family: **CR 400.7** is decorative on
`apply_except_for_type_list_exclusion` — it says nothing about excluding types from a
population; scope it to the `ReturnAll` zone change or drop it. And the EXISTING header
at `oracle_target.rs:8400` cites **CR 205.2b**, which is not in the plan's "do not write
a CR annotation this table does not list" table — either add the row (205.2b, line
1409, "Some objects have more than one card type…") or tell the implementer to drop it
in the U2.2 rewrite.

**G7 [SPOT] "the naive variant" is overloaded across two rows, and V1u-cond's claim is
FALSE against the variant V1u-mono names.** V1u-mono defines the naive variant as
single-pass "widened window AND not an event head". V1u-cond claims to be
revert-failing "against the naive variant, under which the boundary jumps to the FIRST
comma". Under V1u-mono's variant it does NOT: for `"whenever a bird, frog, or otter you
control attacks, draw a card"` the widened window contains the event head `attacks`, so
the whole-window veto fires and the boundary stays at comma 2. V1u-cond is
revert-failing only against the DIFFERENT variant of Theorem 3a (widened window WITHOUT
the event-head exclusion). REPLACEMENT: label them distinctly — "naive-A (single-pass:
widened ∧ ¬event-head)" for V1u-mono and "naive-B (widened, no event-head exclusion)"
for V1u-cond — and state each row's revert target by that label.

**G8 [SPOT] §Pattern Coverage / C5 tail overstates U2's printed-card reach.** "The
single-item family (`except for Merfolk.`) is outside this query and also fixed, plus
the CR 205.3i basic-land-type family (`except for Islands`) that the vocabulary gate
unlocks as a side effect." Measured over the whole corpus, the complete set of distinct
`except for …` clauses is 17, and ZERO are single-item subtype or basic-land-type
unlocks: there is no `except for Islands` printing, and the one `except for basic lands`
printing still declines on the trailing-text guard (`:8479`) both before and after U2
(`"basic"` → `NotSupertype`, then `" lands"` is not a separator). REPLACEMENT: "V3u-land
and the single-item form are BUILDING-BLOCK coverage (zero printed cards today); the
printed-card total for U2 remains the four in C5."

**G9 [SPOT] Anchor nit.** §Step 2 item 5 cites the #5324 core-type sibling as
`:9850`–`:9860`. Measured: doc `:9849`–`:9859`, `#[test]` `:9860`, `fn
trigger_you_cast_oxford_comma_mixed_type_list_spell` `:9861`.

### VERIFIED CLEAN BY THE ROUND-3 REVIEWER — do not re-derive or change

SIZING PASSES: units = 3, scope paths = 7. The reviewer independently walked every file
the plan tells the implementer to edit and found NO eighth path — `game/keywords.rs`
needs no visibility change (`pub mod game` → `pub mod keywords` → `pub fn
source_matches_card_type` at `:659` already reachable from `tests/integration/`);
`oracle_static/mod.rs` is genuinely avoided; `game/scenario.rs` needs no new helper
(Saga chapters are driven in integration tests via `build_resolved_from_def` +
`resolve_ability_chain`, e.g. `tests/integration/issue_2425_fable_chapter_iii_transform.rs`);
`client/public/card-data.json` is gitignored (`.gitignore:39`). SINGLE-PHASE stands.

COMPILE PLAUSIBILITY of every snippet verified: `scan_split_at_phrase` returns
`Option<(&str,&str)>` (prefix, match-start) so `Some((before,_)) => before.trim_end()` /
`None => sentence` is right; `scan_at_word_boundaries` returns `Option<O>` and the
closure's `Ok((tail,false))`/`Ok((tail,true))`/`Err` give exactly the
event-first/predicate-second/advance semantics claimed, and `.unwrap_or(false)` types;
the `|tail: &str|` annotation has a working precedent at `oracle_effect/subject.rs:1789`;
`split_once_on` returns `Err` (not `None`) with `separator: &'a str` accepting a
`'static` literal; `oracle_err` is one-arg (`error.rs:17`);
`TargetFilter::Typed(TypedFilter{type_filters, properties})` makes the U3 applicator
well-typed; `pub(crate) mod oracle_target` makes `crate::parser::oracle_target::apply_…`
reachable from `imperative.rs`; `dest_remainder: &str` and `entry_offset = pos +
phrase_len` (`lower.rs:7495`) means the remainder really begins after the destination
phrase.

COMBINATOR GATE: `FORBIDDEN_METHODS` quoted correctly; no proposed line matches it
(`.contains(&…)` and `trim_end_matches('s')` unmatched by design); family (D) needs ≥4
tag arms so the 3-arm postmodifier `alt` is safe; `**/*_tests.rs` is excluded from the
gate so `.starts_with(", birds")`-style assertions are out of scope; NO allow-marker is
needed anywhere. NOTE the still-open G-finding from round 2 that IS resolved: any
`window.split_once(' ')` must be written `nom_primitives::split_once_on(&window, " ")`
because `\.split_once\(` is listed UNCONDITIONALLY at
`scripts/check-parser-combinators.sh:203`.

U2 PRECEDENCE AND THE MIXED LIST: subtype vocabulary is 530 entries + 24 irregular
plurals; the ONLY collision with `classify_negation`'s keyword set is `sorceries →
Sorcery` (recomputed independently by two reviewers). `artifacts`/`lands` →
`parse_subtype` = None → `Non(Artifact)`/`Non(Land)`; `Phyrexians` →
`Non(Subtype("Phyrexian"))`. Full-consume is guaranteed by `starts_with_word_ci`, so the
omitted `consumed == word.len()` filter really would be dead code.

ALL `[DATA]` PREMISES REPRODUCE EXACTLY: Valley Floodcaller, Immolation Shaman,
Thanos-Copter, Whelming Wave, Slinn Voda, Cyclone Summoner, The Argent Etchings, Tinfoil
Helm (8 ordered Protection entries), Valley Rotcaller (4-leg ObjectCount with Another).

THE `with `-BOUND HOLE WAS HUNTED EXPLICITLY: zero printed cards have an effect-side type
list carrying a `with`/`that`/`which` postmodifier (grep over all trigger descriptions
returns 0), so the bound costs no printed unlock — residual risk 3 should simply be
WIDENED to name `with ` alongside the relative-clause form.

`is_new_sentence_not_type_continuation` has exactly ONE call site; monotonicity (H1)
holds as stated; the #6857 fixture edit is correct in every detail —
`with_subtypes(Vec<&str>) -> &mut Self` and `id() -> ObjectId` exist, the builder chain
matches `:929`–`:944`, and `published_set` really is ascending-id-sorted (`tracked_sets`
does `ids.sort_unstable()`), so appending `floodcaller` LAST is required.

EVERY OTHER ANCHOR IN THE PLAN CHECKS OUT EXACTLY (a ~90-coordinate list was
re-verified). Do not re-measure them.

## Environment notes for whoever resumes

- **Tilt is NOT running** in this container, so the CLAUDE.md "use Tilt logs, never
  cargo" rule inverts: run cargo directly, per the documented fallback
  (`tilt get uiresource clippy >/dev/null 2>&1`; non-zero = down).
- A full `phase-engine` build peaks near **6.6 GB RSS** and takes ~10 min on this
  16 GB box. Use `-j 2`. Two concurrent engine builds OOM-kill each other — that
  happened once already (SIGKILL during `gen-card-data.sh`).
- `./scripts/gen-card-data.sh` takes ~40 min end to end (MTGJSON download + build +
  export) and needs `MTGJSON_SKIP_REFRESH=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1`
  to survive on this box. It produced `client/public/card-data.json` (98 MB,
  gitignored — regenerate, do not look for it in git).
- The export **dirties tracked files** as a side effect. Observed:
  `crates/engine/data/mtgjson-vintage` (a date stamp). CLAUDE.md documents the same
  hazard for `crates/engine/data/oracle-subtypes.json` and
  `crates/engine/data/known-tokens.toml`. Restore them; never `git add -A`.
- Baseline artifacts for the corpus diff were written to the session scratchpad
  (`card-data.BASE.json`, `ast-digest.BASE.jsonl`). **These are lost when the
  container is reclaimed** — regenerate from `BASE_SHA` before trusting any
  before/after comparison.
- `docs/MagicCompRules.txt` is gitignored; run `./scripts/fetch-comp-rules.sh`
  (works, ~1 min) before doing any CR verification.

## GitHub access constraint (affects the PR step)

This session's GitHub **API** access is scoped to the fork `alicewonderland-dev/phase`.
`phase-rs/phase` can be read over git and over the web, but `add_repo` refuses to
attach it ("cross-tier adds are not supported in v1"), and both `api.github.com` via
curl and WebFetch are blocked by the proxy. Consequences:

- Issue and PR *reading* on `phase-rs/phase` works via WebFetch against the HTML pages.
- `git push` to the fork works. Pushing to `phase-rs/phase` does not.
- Opening a PR **against `phase-rs/phase` was not attempted yet** and is expected to
  fail through the GitHub MCP tools, which refuse out-of-scope repositories. If it
  does, the fallback is to push the branch to the fork and report, rather than
  silently opening the PR somewhere else.

## Next actions on resume

1. Re-dispatch round 4 of the plan-review loop: fresh Opus planner with
   `plan-7451.md` + the G1–G9 findings above; then a fresh Opus reviewer.
2. Continue the loop until a review round returns zero gaps. Do not stop at "two
   rounds and ship" — the loop is unbounded by design, and rounds 1–3 each caught a
   real defect (a non-monotone design, a printed-card regression, an unsound proof).
3. Then Steps 3–7: `engine-implementation-executor` (Sonnet) → checkpoint commit →
   verification at the committed candidate → `/review-impl` (Opus) → acceptance.
4. **Delete `.wip-7451/` in the final commit** before opening the PR.
