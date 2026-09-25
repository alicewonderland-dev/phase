import { createStore, get, set } from "idb-keyval";

const SAVED_DECK_LIBRARY_LOCK = "phase-saved-deck-library";
/** localStorage key holding the last-published saved-deck library generation. */
const SAVED_DECK_GENERATION_KEY = "phase-saved-deck-library-generation";

export const LOCK_WAIT_TIMEOUT_MS = 2000;
export const GENERATION_IO_TIMEOUT_MS = 1000;
export const LIBRARY_VIEW_TIMEOUT_MS = 2000;

declare const savedDeckTxnBrand: unique symbol;
/** Proof that the holder runs inside a saved-deck library transaction. */
export type SavedDeckTxn = { readonly [savedDeckTxnBrand]: true };
const TXN = {} as SavedDeckTxn; // module-private; the brand exists only at compile time

export type SavedDeckTxnSkipReason =
  | "lock-unavailable"
  | "lock-refused"
  | "lock-timeout"
  | "library-view-unconfirmed"
  | "generation-unpublished";

export type SavedDeckTxnResult<T> =
  | { status: "committed"; value: T }
  | { status: "skipped"; reason: SavedDeckTxnSkipReason };

let _generationStore: ReturnType<typeof createStore> | null = null;
function generationStore(): ReturnType<typeof createStore> {
  if (!_generationStore) {
    _generationStore = createStore("phase-saved-deck-library", "generation");
  }
  return _generationStore;
}

type Bounded<T> = { settled: true; value: T } | { settled: false };

/** Race `start()` against `ms`; a synchronous throw from `start` becomes a rejection. */
function within<T>(start: () => Promise<T>, ms: number): Promise<Bounded<T>> {
  return new Promise<Bounded<T>>((resolve) => {
    let done = false;
    const timer = setTimeout(() => {
      if (done) return;
      done = true;
      resolve({ settled: false });
    }, ms);
    Promise.resolve()
      .then(start)
      .then(
        (value) => {
          if (done) return;
          done = true;
          clearTimeout(timer);
          resolve({ settled: true, value });
        },
        () => {
          if (done) return;
          done = true;
          clearTimeout(timer);
          resolve({ settled: false });
        },
      );
  });
}

export type SavedDeckTxnGatePhase = "draft-autosave-after-owner-selection" | "builder-save-after-data-write";
let gateForTests: ((phase: SavedDeckTxnGatePhase) => Promise<void> | void) | null = null;
let lockWaitForTests: number | null = null;

/** Narrow test seam: production builds never invoke a transaction gate. */
export function setSavedDeckTxnGateForTests(gate: typeof gateForTests): void {
  gateForTests = gate;
}
/** Narrow test seam: production builds always use LOCK_WAIT_TIMEOUT_MS. */
export function setSavedDeckTxnLockWaitForTests(ms: number | null): void {
  lockWaitForTests = ms;
}
export function savedDeckTxnGate(phase: SavedDeckTxnGatePhase): Promise<void> | void {
  if (import.meta.env.MODE === "test") return gateForTests?.(phase);
}

function lockWaitMs(): number {
  return import.meta.env.MODE === "test" && lockWaitForTests !== null ? lockWaitForTests : LOCK_WAIT_TIMEOUT_MS;
}

/**
 * Wait until this tab's copy of the library reflects every committed transaction, or give up
 * after LIBRARY_VIEW_TIMEOUT_MS.
 */
function awaitLibraryView(committed: number): Promise<boolean> {
  if (Number(localStorage.getItem(SAVED_DECK_GENERATION_KEY) ?? 0) >= committed) return Promise.resolve(true);
  return new Promise<boolean>((resolve) => {
    let done = false;
    const finish = (result: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      clearInterval(backstop);
      window.removeEventListener("storage", onStorage);
      resolve(result);
    };
    const check = () => {
      if (Number(localStorage.getItem(SAVED_DECK_GENERATION_KEY) ?? 0) >= committed) finish(true);
    };
    const onStorage = (event: StorageEvent) => {
      if (event.key === SAVED_DECK_GENERATION_KEY || event.key === null) check();
    };
    window.addEventListener("storage", onStorage);
    const backstop = setInterval(check, 25);
    const timer = setTimeout(() => finish(false), LIBRARY_VIEW_TIMEOUT_MS);
  });
}

type BarrierOutcome =
  // skip policy only: nothing was published, nothing is armed.
  | { armed: false; reason: SavedDeckTxnSkipReason }
  // `next` must be published in a `finally` regardless of what happens next.
  | { armed: true; next: number; proceed: boolean; reason?: SavedDeckTxnSkipReason };

/** `policy: "skip"` may return `armed: false`, giving up before publishing; `policy: "proceed"` always arms. */
async function runLibraryBarrier(policy: "proceed" | "skip"): Promise<BarrierOutcome> {
  const read = await within(() => get<number>("generation", generationStore()), GENERATION_IO_TIMEOUT_MS);
  // Treating an empty store as unconfirmed would make every autosave skip on a fresh library, so
  // only a read that fails to settle (I/O failure or timeout) is genuinely unconfirmed.
  const committed = !read.settled ? null : typeof read.value === "number" ? read.value : 0;
  if (committed === null && policy === "skip") {
    return { armed: false, reason: "library-view-unconfirmed" };
  }
  const target = committed ?? 0;
  if (committed !== null) {
    const caughtUp = await awaitLibraryView(committed);
    if (!caughtUp && policy === "skip") {
      return { armed: false, reason: "library-view-unconfirmed" };
    }
  }
  const next = Math.max(Number(localStorage.getItem(SAVED_DECK_GENERATION_KEY) ?? 0), target) + 1;
  const published = await within(() => set("generation", next, generationStore()), GENERATION_IO_TIMEOUT_MS);
  if (!published.settled) {
    return { armed: true, next, proceed: policy === "proceed", reason: "generation-unpublished" };
  }
  return { armed: true, next, proceed: true };
}

function publishLocalGeneration(next: number): void {
  try {
    localStorage.setItem(SAVED_DECK_GENERATION_KEY, String(next));
  } catch (error) {
    console.warn("[savedDeckTransaction] failed to publish local generation:", error);
  }
}

/**
 * Run `body` under the saved-deck library lock and the write-ahead generation barrier, skipping
 * instead of running `body` when the lock or the barrier could not be confirmed. The autosave
 * uses this policy: a skip never overwrites a deck it could not safely observe or announce.
 */
export async function withSavedDeckLibraryOrSkip<T>(
  body: (txn: SavedDeckTxn) => T | Promise<T>,
): Promise<SavedDeckTxnResult<T>> {
  const locks = globalThis.navigator?.locks ?? null;
  if (!locks) return { status: "skipped", reason: "lock-unavailable" };

  const controller = new AbortController();
  let granted = false;
  let timedOut = false;
  const waitMs = lockWaitMs();
  const timer = Number.isFinite(waitMs)
    ? setTimeout(() => {
        timedOut = true;
        controller.abort();
      }, waitMs)
    : null;

  try {
    return await locks.request(SAVED_DECK_LIBRARY_LOCK, { mode: "exclusive", signal: controller.signal }, async () => {
      granted = true;
      if (timer) clearTimeout(timer);
      const outcome = await runLibraryBarrier("skip");
      if (!outcome.armed) {
        return { status: "skipped", reason: outcome.reason } as SavedDeckTxnResult<T>;
      }
      try {
        if (!outcome.proceed) {
          return { status: "skipped", reason: outcome.reason! } as SavedDeckTxnResult<T>;
        }
        const value = await body(TXN);
        return { status: "committed", value } as SavedDeckTxnResult<T>;
      } finally {
        publishLocalGeneration(outcome.next);
      }
    });
  } catch (error) {
    if (timer) clearTimeout(timer);
    if (granted) throw error;
    return { status: "skipped", reason: timedOut ? "lock-timeout" : "lock-refused" };
  }
}

/**
 * Run `body` under the saved-deck library lock and the write-ahead generation barrier, always
 * running the body — unguarded, if the lock could not be acquired or the barrier could not
 * confirm this tab's view. Manual/user-initiated writers use this policy: they must never block
 * indefinitely behind another tab.
 */
export async function withSavedDeckLibrary<T>(body: (txn: SavedDeckTxn) => T | Promise<T>): Promise<T> {
  const locks = globalThis.navigator?.locks ?? null;
  if (!locks) return body(TXN);

  const controller = new AbortController();
  let granted = false;
  const waitMs = lockWaitMs();
  const timer = Number.isFinite(waitMs)
    ? setTimeout(() => {
        controller.abort();
      }, waitMs)
    : null;

  try {
    return await locks.request(SAVED_DECK_LIBRARY_LOCK, { mode: "exclusive", signal: controller.signal }, async () => {
      granted = true;
      if (timer) clearTimeout(timer);
      const outcome = await runLibraryBarrier("proceed");
      try {
        return await body(TXN);
      } finally {
        if (outcome.armed) publishLocalGeneration(outcome.next);
      }
    });
  } catch (error) {
    if (timer) clearTimeout(timer);
    if (granted) throw error;
    return body(TXN);
  }
}
