//! Deterministic decision-cost regression gate.
//!
//! Runs the `default_scenarios` matchups, or the `--scenario` selection, through
//! a fixed seeded action-cap prefix, field-wise sums the engine perf counters,
//! and compares the integer payload against a committed baseline. Catches
//! cost-per-decision regressions (clone storms, quadratic combat scans, display
//! sweeps in search) that the win-rate `cargo ai-gate` is structurally blind to.
//!
//! Workload (seed, action_cap) is fixed by compile-time consts in
//! `duel_suite::perf`, never flags.
//!
//! Individual trajectories are NOT cross-process deterministic — engine
//! HashSet/HashMap iteration order leaks per-process RandomState into AI
//! tie-breaking (issue #4878). The gate therefore aggregates the per-counter
//! MEDIAN over `PERF_SAMPLE_COUNT` INDEPENDENT cold child processes (fresh
//! RandomState each), spawned via `current_exe()` with the internal
//! `--emit-sample` flag. `main()` dispatches three mutually exclusive modes:
//! child (emit one sample), repro-report (margin gate over saved runs), and
//! parent gate (spawn K children, median, compare).

// pod-lab loop-3 Q5: native-binary throughput lever, gated in Cargo.toml so
// wasm32 builds of this crate's lib (pulled in by engine-wasm/draft-wasm)
// never see it.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use engine::database::CardDatabase;
use phase_ai::duel_suite::perf::{
    compare, default_scenarios, load_report, median_report, print_markdown, print_repro_margin,
    render_error_markdown, repro_margin_report, resolve_scenarios, run_perf_suite, PerfReport,
    PERF_ACTION_CAP, PERF_BASE_SEED, PERF_SAMPLE_COUNT,
};
use phase_ai::duel_suite::{find_matchup, resolve_deck_ref};

const DEFAULT_BASELINE: &str = "crates/phase-ai/baselines/perf-baseline.json";
const DEFAULT_CURRENT: &str = "target/ai-perf-gate-current.json";

/// `run_perf_suite`'s AI search recurses deeper than the platform default
/// thread stack on Windows (confirmed: every child overflows immediately
/// after game start under this crate's release profile). Same root cause and
/// fix as `ai_commander.rs`'s `GAME_THREAD_STACK_SIZE` / `duel_suite::run`'s
/// identical spawn — this binary just never got the fix applied.
const PERF_THREAD_STACK_SIZE: usize = 32 << 20;

#[derive(Debug)]
struct Args {
    data_root: PathBuf,
    baseline: PathBuf,
    current_output: PathBuf,
    refresh_baseline: bool,
    /// Internal: emit a single-trajectory sample to this path and exit. Set only
    /// on the K child processes the parent spawns.
    emit_sample: Option<PathBuf>,
    /// Internal: run the reproducibility MARGIN gate over `--repro-input` reports.
    repro_report: bool,
    /// Internal: the validation-run reports the margin gate aggregates (repeatable).
    repro_inputs: Vec<PathBuf>,
    /// The scenario list passed to `run_perf_suite`; defaults to `default_scenarios()`.
    scenarios: Vec<&'static str>,
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
            print_usage();
            std::process::exit(2);
        }
    };

    // Branch 1 — child: load the DB, emit ONE single-trajectory sample, exit.
    if args.emit_sample.is_some() {
        run_child(&args);
        return;
    }

    // Branch 2 — repro-report: pure aggregation over saved reports, no DB load.
    if args.repro_report {
        run_repro_report(&args.baseline, &args.repro_inputs);
        return;
    }

    // Branch 3 — parent gate: spawn K children, take the per-counter median, stamp
    // provenance, compare (or refresh). Never loads the DB itself.
    run_parent_gate(&args);
}

/// Branch 1 dispatch: load the DB, emit ONE single-trajectory sample to the
/// file, exit. Emits NOTHING on stdout (GAP 4) so the parent's stdout stays a
/// clean table; diagnostics go to stderr only. Runs on a large-stack thread
/// (see `PERF_THREAD_STACK_SIZE`) since the AI search recurses past the
/// platform default; an unhandled panic there would otherwise unwind only
/// the spawned thread and exit 0 silently, so a join failure is mapped to
/// exit 101 (mirrors `ai_commander.rs`'s identical convention). Split out of
/// `main` so `main` names no scenario list anywhere.
fn run_child(args: &Args) {
    let data_root = args.data_root.clone();
    let sample_path = args
        .emit_sample
        .clone()
        .expect("run_child requires emit_sample to be set");
    let scenarios = args.scenarios.clone();
    let handle = std::thread::Builder::new()
        .name("ai-perf-gate-sample".to_string())
        .stack_size(PERF_THREAD_STACK_SIZE)
        .spawn(move || run_child_sample(&data_root, &sample_path, &scenarios))
        .expect("failed to spawn perf-sample thread");
    if handle.join().is_err() {
        std::process::exit(101);
    }
}

/// Branch 1: emit a single-trajectory sample report to `sample_path`. Loads the
/// card DB (the only branch that does). Never writes stdout.
fn run_child_sample(data_root: &Path, sample_path: &Path, scenarios: &[&str]) {
    let db_path = data_root.join("card-data.json");
    let db = match CardDatabase::from_export(&db_path) {
        Ok(db) => db,
        Err(err) => {
            eprintln!(
                "failed to load card database from {}: {err}",
                db_path.display()
            );
            std::process::exit(2);
        }
    };
    let report = run_perf_suite(&db, PERF_BASE_SEED, PERF_ACTION_CAP, scenarios);
    if let Err(err) = write_report(&report, sample_path) {
        eprintln!(
            "failed to write sample report {}: {err}",
            sample_path.display()
        );
        std::process::exit(2);
    }
}

/// Branch 2: the reproducibility MARGIN gate. Exit 0 iff every counter's worst
/// observed value across the validation runs stays within the named fraction of
/// its FAIL headroom. This exit code IS the M15 margin gate.
fn run_repro_report(baseline_path: &Path, repro_inputs: &[PathBuf]) {
    let baseline = match load_report(baseline_path) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("failed to load baseline {}: {err}", baseline_path.display());
            std::process::exit(2);
        }
    };
    let mut runs = Vec::with_capacity(repro_inputs.len());
    for path in repro_inputs {
        match load_report(path) {
            Ok(report) => runs.push(report),
            Err(err) => {
                eprintln!("failed to load repro input {}: {err}", path.display());
                std::process::exit(2);
            }
        }
    }
    let margin = repro_margin_report(&baseline, &runs);
    print_repro_margin(&margin);
    if margin.all_within_margin() {
        std::process::exit(0);
    }
    std::process::exit(1);
}

/// Branch 3: spawn `PERF_SAMPLE_COUNT` independent cold child processes, aggregate
/// the per-counter median, stamp provenance, then refresh-or-compare.
fn run_parent_gate(args: &Args) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("failed to resolve current executable for sampling: {err}");
            std::process::exit(2);
        }
    };
    // Spawn K children SEQUENTIALLY (blocking .status()); each is an independent
    // process with a fresh std RandomState, hence an independent trajectory.
    let mut samples = Vec::with_capacity(PERF_SAMPLE_COUNT);
    let mut temp_paths = Vec::with_capacity(PERF_SAMPLE_COUNT);
    for i in 0..PERF_SAMPLE_COUNT {
        let tmp_i =
            std::env::temp_dir().join(format!("ai-perf-sample-{}-{i}.json", std::process::id()));
        // Registered BEFORE the spawn so every failure path below cleans it up.
        temp_paths.push(tmp_i.clone());
        let status = Command::new(&exe)
            .args(child_sample_args(&tmp_i, &args.data_root, &args.scenarios))
            .stdout(Stdio::null()) // GAP 4: parent's stdout stays a clean table
            .stderr(Stdio::inherit()) // child diagnostics still visible in CI logs
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("perf sample child {i} exited with status {s} — aborting (no silent K reduction)");
                cleanup_temps(&temp_paths);
                std::process::exit(2);
            }
            Err(err) => {
                eprintln!("failed to spawn perf sample child {i}: {err}");
                cleanup_temps(&temp_paths);
                std::process::exit(2);
            }
        }
        match load_report(&tmp_i) {
            Ok(report) => samples.push(report),
            Err(err) => {
                eprintln!(
                    "perf sample child {i} produced an unreadable report {}: {err}",
                    tmp_i.display()
                );
                cleanup_temps(&temp_paths);
                std::process::exit(2);
            }
        }
    }

    let mut current = median_report(&samples);
    // Stamp provenance the parent can compute without loading the DB.
    current.git_sha = command_output("git", &["rev-parse", "--short=12", "HEAD"]);
    current.card_data_hash = gate_card_data_hash(&args.data_root, &args.scenarios);

    eprintln!(
        "perf suite: seed={} action_cap={} sample_count={} scenarios={:?} wall_clock={}ms",
        current.base_seed,
        current.action_cap,
        current.sample_count,
        current.scenarios,
        current.wall_clock_ms
    );

    if let Err(err) = write_report(&current, &args.current_output) {
        eprintln!(
            "failed to write current report {}: {err}",
            args.current_output.display()
        );
        cleanup_temps(&temp_paths);
        std::process::exit(2);
    }

    if args.refresh_baseline {
        if args.baseline.exists() {
            match load_report(&args.baseline).and_then(|baseline| compare(&baseline, &current)) {
                Ok(report) => print_markdown(&report),
                Err(err) => eprintln!("could not compare old baseline: {err}"),
            }
        }
        if let Err(err) = write_report(&current, &args.baseline) {
            eprintln!(
                "failed to write baseline {}: {err}",
                args.baseline.display()
            );
            cleanup_temps(&temp_paths);
            std::process::exit(2);
        }
        eprintln!("baseline refreshed at {}", args.baseline.display());
        cleanup_temps(&temp_paths);
        return;
    }

    // Both bail-outs print the refusal to STDOUT as well as stderr. The workflow redirects
    // stdout into `target/ai-perf-gate-report.md` and posts it as a drift issue, and nothing
    // on the path above this point writes to stdout — so a stderr-only refusal left that file
    // at zero bytes and the workflow answered it with "failed without a drift report" and no
    // issue. The exit code and a non-empty body are needed TOGETHER; either alone posts nothing.
    let baseline = match load_report(&args.baseline) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("failed to load baseline {}: {err}", args.baseline.display());
            print!("{}", render_error_markdown(&err));
            cleanup_temps(&temp_paths);
            std::process::exit(2);
        }
    };

    let report = match compare(&baseline, &current) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("compare failed: {err}");
            print!("{}", render_error_markdown(&err));
            cleanup_temps(&temp_paths);
            std::process::exit(2);
        }
    };
    print_markdown(&report);
    cleanup_temps(&temp_paths);
    if report.any_fail() {
        std::process::exit(1);
    }
}

/// Build the argv the parent passes to an `--emit-sample` child: the sample and
/// data-root paths, followed by one `--scenario ID` per requested scenario. The
/// ids are ALWAYS forwarded, the defaults included, so the default CI run
/// exercises the same argv-forwarding path as an override.
fn child_sample_args(sample_path: &Path, data_root: &Path, scenarios: &[&str]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "--emit-sample".into(),
        sample_path.into(),
        "--data-root".into(),
        data_root.into(),
    ];
    for id in scenarios {
        args.push("--scenario".into());
        args.push((*id).into());
    }
    args
}

/// Best-effort removal of the per-run temp sample files (ignore errors).
fn cleanup_temps(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn parse_args(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut data_root = std::env::var("PHASE_CARDS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));
    let mut baseline: Option<PathBuf> = None;
    let mut current_output = PathBuf::from(DEFAULT_CURRENT);
    let mut refresh_baseline = false;
    let mut emit_sample = None;
    let mut repro_report = false;
    let mut repro_inputs = Vec::new();
    let mut requested: Option<Vec<String>> = None;

    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--data-root" => data_root = next_path(&mut iter, "--data-root")?,
            "--baseline" => baseline = Some(next_path(&mut iter, "--baseline")?),
            "--current-output" => current_output = next_path(&mut iter, "--current-output")?,
            "--refresh-baseline" => refresh_baseline = true,
            "--emit-sample" => emit_sample = Some(next_path(&mut iter, "--emit-sample")?),
            "--repro-report" => repro_report = true,
            "--repro-input" => repro_inputs.push(next_path(&mut iter, "--repro-input")?),
            "--scenario" => requested.get_or_insert_with(Vec::new).extend(
                next_value(&mut iter, "--scenario")?
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
            ),
            "--help" | "-h" => return Err(String::new()),
            _ => return Err(format!("unknown option: {arg}")),
        }
    }

    if requested.is_some() && repro_report {
        return Err("--scenario has no effect with --repro-report, which runs no suite".into());
    }
    if requested.is_some() && emit_sample.is_none() && !repro_report && baseline.is_none() {
        return Err("--scenario requires an explicit --baseline PATH: a non-default scenario set cannot be compared against, or refreshed into, the committed baseline".into());
    }
    let scenarios = match &requested {
        None => default_scenarios(),
        Some(ids) => resolve_scenarios(ids).map_err(|e| format!("--scenario: {e}"))?,
    };

    Ok(Args {
        data_root,
        baseline: baseline.unwrap_or_else(|| PathBuf::from(DEFAULT_BASELINE)),
        current_output,
        refresh_baseline,
        emit_sample,
        repro_report,
        repro_inputs,
        scenarios,
    })
}

fn next_path(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    next_value(iter, flag).map(PathBuf::from)
}

fn next_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Provenance hash over ONLY the `card-data.json` entries this gate's scenarios
/// actually consume.
///
/// This was previously `git hash-object data/card-data.json` — the whole file,
/// ~35.6k cards. The three scenarios in [`default_scenarios`] draw from
/// committed, frozen decks (inline builders plus pinned snapshots) naming ~46.
/// `card-data.json` is a DERIVED artifact of both MTGJSON and the Oracle parser,
/// so every unrelated set release *and every parser change* moved the stamp.
/// `PerfCompareReport::card_data_changed` was therefore true on nearly every
/// run, which made it useless for the one judgement it exists to support:
/// telling a genuine cost-per-node regression (hashes equal) apart from a
/// card-data-driven trajectory shift (hashes differ).
///
/// Measured over five local card-data vintages spanning 2026-07-25..2026-08-04,
/// one pair differing only by an Oracle-parser change and not by MTGJSON: five
/// distinct whole-file hashes, exactly ONE distinct gate-subset hash.
///
/// Still a `git hash-object` blob SHA, so the field's format and meaning are
/// unchanged and no hashing dependency enters this crate. Re-serializing is
/// canonical: this workspace leaves serde_json's `preserve_order` off, so object
/// keys serialize sorted (documented at `engine/src/bin/set_check.rs`), and the
/// `BTreeMap`s below fix the order of everything above them. A card a deck names
/// but `card-data.json` lacks is simply absent from the subset — which still
/// moves the hash, since the key set shrinks.
///
/// Returns `None` on any failure, matching [`command_output`]'s convention:
/// `card_data_changed()` then reports false rather than inventing a delta. Every
/// failure path announces itself on stderr — an unstamped run must not read as a
/// clean one.
fn gate_card_data_hash(data_root: &Path, scenarios: &[&str]) -> Option<String> {
    // DB-free by construction: `resolve_deck_ref` expands inline builders and
    // pinned snapshots without a `CardDatabase`, preserving this branch's
    // documented never-loads-the-DB property.
    let mut names = BTreeSet::new();
    for id in scenarios {
        let Some(matchup) = find_matchup(id) else {
            eprintln!("provenance: scenario {id:?} does not resolve — card-data hash unstamped");
            return None;
        };
        for deck in [&matchup.p0, &matchup.p1] {
            match resolve_deck_ref(deck) {
                // The engine keys `card-data.json` by Rust `to_lowercase()`.
                Ok(cards) => names.extend(cards.into_iter().map(|c| c.to_lowercase())),
                Err(err) => {
                    eprintln!(
                        "provenance: deck {deck:?} failed to resolve ({err}) — card-data hash unstamped"
                    );
                    return None;
                }
            }
        }
    }

    let db_path = data_root.join("card-data.json");
    let db: BTreeMap<String, serde_json::Value> = match std::fs::read_to_string(&db_path)
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
    {
        Ok(db) => db,
        Err(err) => {
            eprintln!(
                "provenance: could not read {} ({err}) — card-data hash unstamped",
                db_path.display()
            );
            return None;
        }
    };
    let subset: BTreeMap<&str, &serde_json::Value> = names
        .iter()
        .filter_map(|name| db.get(name).map(|entry| (name.as_str(), entry)))
        .collect();

    // `git hash-object` needs a path, so the canonical subset goes through a
    // temp file. Removed on every exit path, including the failures below.
    let tmp = std::env::temp_dir().join(format!("ai-perf-gate-cards-{}.json", std::process::id()));
    let hash = match File::create(&tmp)
        .map_err(|e| e.to_string())
        .and_then(|file| {
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &subset).map_err(|e| e.to_string())?;
            // `BufWriter` discards errors from its drop-time flush, so a failed
            // final write would leave a TRUNCATED subset that `git hash-object`
            // still hashes — yielding a well-formed provenance stamp for content
            // that was never written. Flush explicitly and fail closed instead.
            writer.flush().map_err(|e| e.to_string())
        }) {
        Ok(()) => match tmp.to_str() {
            Some(path) => command_output("git", &["hash-object", path]),
            None => {
                eprintln!("provenance: temp path is not valid UTF-8 — card-data hash unstamped");
                None
            }
        },
        Err(err) => {
            eprintln!(
                "provenance: could not write the card subset ({err}) — card-data hash unstamped"
            );
            None
        }
    };
    let _ = std::fs::remove_file(&tmp);

    if hash.is_some() {
        eprintln!(
            "provenance: card-data hash covers {} of {} deck-named cards from scenarios {:?}",
            subset.len(),
            names.len(),
            scenarios
        );
    }
    hash
}

fn write_report(report: &PerfReport, path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), report).map_err(std::io::Error::other)
}

fn print_usage() {
    eprintln!("Usage: cargo ai-perf-gate [--refresh-baseline]");
    eprintln!(
        "                          [--data-root DIR] [--baseline PATH] [--current-output PATH]"
    );
    eprintln!(
        "                          [--scenario ID[,ID...] (repeatable; requires --baseline)]"
    );
    eprintln!();
    eprintln!("The gate runs PERF_SAMPLE_COUNT independent sample processes and compares the");
    eprintln!("per-counter median against the committed baseline (issue #4878).");
    eprintln!();
    eprintln!("Internal flags (spawned/orchestrated automatically, not for manual use):");
    eprintln!("  --emit-sample PATH   emit one single-trajectory sample to PATH and exit");
    eprintln!(
        "  --repro-report       run the reproducibility MARGIN gate over --repro-input reports"
    );
    eprintln!("  --repro-input PATH   a validation-run report for --repro-report (repeatable)");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A scratch dir per `tag`, pre-cleaned so a pid-reused leftover from a prior
    /// failed run does not collide (mirrors `refresh_baseline_cli.rs::scratch`).
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ai-perf-gate-bin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// A valid, empty card database (`{}` deserialises as a card-name→entry map with
    /// no entries). Returns the data-root path.
    fn empty_card_db(dir: &Path) -> PathBuf {
        let root = dir.join("cards");
        std::fs::create_dir_all(&root).expect("create data root");
        std::fs::write(root.join("card-data.json"), "{}").expect("write card data");
        root
    }

    #[test]
    fn no_scenario_flag_selects_the_default_scenarios() {
        let parsed = parse_args(args(&[])).expect("parse");
        assert_eq!(parsed.scenarios, default_scenarios());
        assert_eq!(parsed.baseline, PathBuf::from(DEFAULT_BASELINE));
    }

    #[test]
    fn an_unknown_scenario_is_a_parse_error() {
        let err = parse_args(args(&[
            "--scenario",
            "no-such-matchup",
            "--baseline",
            "b.json",
        ]))
        .expect_err("unknown scenario id must be a parse error");
        assert!(
            err.contains("no-such-matchup"),
            "error should name the bad id, got: {err}"
        );
    }

    #[test]
    fn scenario_flags_accumulate_across_repeats_and_commas() {
        let parsed = parse_args(args(&[
            "--scenario",
            "azorius-vs-prowess",
            "--scenario",
            "red-mirror, affinity-mirror",
            "--baseline",
            "b.json",
        ]))
        .expect("parse");
        assert_eq!(
            parsed.scenarios,
            vec!["azorius-vs-prowess", "red-mirror", "affinity-mirror"]
        );
    }

    #[test]
    fn a_scenario_flag_naming_nothing_is_a_parse_error() {
        parse_args(args(&["--scenario", "", "--baseline", "b.json"]))
            .expect_err("an empty --scenario value must be a parse error");
        parse_args(args(&["--scenario", " , ", "--baseline", "b.json"]))
            .expect_err("a --scenario value with only blanks must be a parse error");
    }

    #[test]
    fn a_scenario_selection_requires_an_explicit_baseline() {
        let err = parse_args(args(&["--scenario", "red-mirror"]))
            .expect_err("--scenario with no --baseline must be a parse error");
        assert!(
            err.contains("--baseline"),
            "error should mention --baseline, got: {err}"
        );
        let err = parse_args(args(&["--scenario", "red-mirror", "--refresh-baseline"]))
            .expect_err("--scenario with --refresh-baseline and no --baseline must be refused");
        assert!(
            err.contains("--baseline"),
            "error should mention --baseline, got: {err}"
        );
        // paired positive: the same selection WITH --baseline parses fine.
        parse_args(args(&["--scenario", "red-mirror", "--baseline", "b.json"])).expect("parse");
    }

    #[test]
    fn a_scenario_selection_is_refused_with_repro_report() {
        parse_args(args(&[
            "--repro-report",
            "--scenario",
            "red-mirror",
            "--baseline",
            "b.json",
        ]))
        .expect_err("--scenario with --repro-report must be a parse error");
        // paired positive: --repro-report alone (no --scenario) still parses.
        parse_args(args(&["--repro-report", "--baseline", "b.json"])).expect("parse");
    }

    #[test]
    fn child_sample_args_round_trip_the_scenario_list() {
        let sample_path = PathBuf::from("/tmp/sample.json");
        let data_root = PathBuf::from("/tmp/cards");
        let raw = child_sample_args(&sample_path, &data_root, &["azorius-vs-prowess"]);
        let strings: Vec<String> = raw
            .into_iter()
            .map(|s| s.into_string().expect("argv is UTF-8"))
            .collect();
        let parsed = parse_args(strings).expect("child argv must parse without --baseline");
        assert_eq!(parsed.emit_sample, Some(sample_path.clone()));
        assert_eq!(parsed.scenarios, vec!["azorius-vs-prowess"]);

        // the same holds for the default scenario list.
        let raw = child_sample_args(&sample_path, &data_root, &default_scenarios());
        let strings: Vec<String> = raw
            .into_iter()
            .map(|s| s.into_string().expect("argv is UTF-8"))
            .collect();
        let parsed = parse_args(strings).expect("child argv must parse without --baseline");
        assert_eq!(parsed.scenarios, default_scenarios());
    }

    #[test]
    fn a_selected_scenario_reaches_run_perf_suite_in_place_of_the_defaults() {
        let dir = scratch("b8");
        let data_root = empty_card_db(&dir);
        let sample_path = dir.join("sample.json");
        let args = parse_args(args(&[
            "--emit-sample",
            sample_path.to_str().unwrap(),
            "--data-root",
            data_root.to_str().unwrap(),
            "--scenario",
            "azorius-vs-prowess",
        ]))
        .expect("parse");

        run_child(&args);

        let report = load_report(&sample_path).expect("child must write a readable report");
        assert_eq!(report.scenarios, vec!["azorius-vs-prowess".to_string()]);
        // Reach guard: proves the suite actually ran rather than writing an empty
        // report. `azorius-vs-prowess` was not yet probed on a `{}` card DB
        // before this test; the assertion below establishes it produces
        // non-zero counters, the same shape as the default scenarios on `{}`.
        assert!(
            report.counters.0.values().any(|v| *v > 0),
            "expected at least one non-zero perf counter, got {:?}",
            report.counters
        );
    }

    #[test]
    fn the_provenance_hash_covers_only_the_selected_scenarios() {
        // Pick one card name that appears only in red-mirror's decks and one that
        // appears only in affinity-mirror's decks, so the two scenario selections
        // stamp genuinely different card-data subsets.
        let red = find_matchup("red-mirror").expect("red-mirror must resolve");
        let affinity = find_matchup("affinity-mirror").expect("affinity-mirror must resolve");
        let red_names: BTreeSet<String> = [&red.p0, &red.p1]
            .into_iter()
            .flat_map(|d| resolve_deck_ref(d).expect("resolve red-mirror deck"))
            .map(|c| c.to_lowercase())
            .collect();
        let affinity_names: BTreeSet<String> = [&affinity.p0, &affinity.p1]
            .into_iter()
            .flat_map(|d| resolve_deck_ref(d).expect("resolve affinity-mirror deck"))
            .map(|c| c.to_lowercase())
            .collect();
        let red_only = red_names
            .difference(&affinity_names)
            .next()
            .expect("red-mirror must name a card affinity-mirror does not")
            .clone();
        let affinity_only = affinity_names
            .difference(&red_names)
            .next()
            .expect("affinity-mirror must name a card red-mirror does not")
            .clone();

        let dir = scratch("b9");
        let data_root = dir.join("cards");
        std::fs::create_dir_all(&data_root).expect("create data root");
        let mut db = serde_json::Map::new();
        db.insert(red_only, serde_json::json!({}));
        db.insert(affinity_only, serde_json::json!({}));
        std::fs::write(
            data_root.join("card-data.json"),
            serde_json::to_string(&db).unwrap(),
        )
        .expect("write card data");

        let red_hash = gate_card_data_hash(&data_root, &["red-mirror"]);
        let affinity_hash = gate_card_data_hash(&data_root, &["affinity-mirror"]);
        assert!(red_hash.is_some(), "red-mirror hash must be stamped");
        assert!(
            affinity_hash.is_some(),
            "affinity-mirror hash must be stamped"
        );
        assert_ne!(
            red_hash, affinity_hash,
            "provenance hash must scope to the selected scenarios only"
        );
    }
}
