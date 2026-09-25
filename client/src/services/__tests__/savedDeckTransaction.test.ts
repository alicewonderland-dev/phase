import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  GENERATION_IO_TIMEOUT_MS,
  LIBRARY_VIEW_TIMEOUT_MS,
  LOCK_WAIT_TIMEOUT_MS,
  setSavedDeckTxnLockWaitForTests,
  withSavedDeckLibrary,
  withSavedDeckLibraryOrSkip,
} from "../savedDeckTransaction";
import {
  installFifoWebLocks,
  readIdbGenerationForTests,
  refusingWebLocks,
  resetSavedDeckLibraryForTests,
  seedGenerationForTests,
  uninstallWebLocks,
} from "../../test/helpers/webLocks";

const GENERATION_KEY = "phase-saved-deck-library-generation";

beforeEach(async () => {
  installFifoWebLocks();
  await resetSavedDeckLibraryForTests();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  uninstallWebLocks();
});

describe("withSavedDeckLibrary / withSavedDeckLibraryOrSkip", () => {
  it("runs the body while the lock is held", async () => {
    let observedHeld: unknown[] = [];
    await withSavedDeckLibrary(async () => {
      observedHeld = (await navigator.locks.query()).held ?? [];
    });
    expect(observedHeld).toEqual([{ name: "phase-saved-deck-library", mode: "exclusive" }]);
  });

  it("with no lock manager, the proceed policy runs the body synchronously and the skip policy skips", async () => {
    uninstallWebLocks();
    let ran = false;
    const promise = withSavedDeckLibrary(() => {
      ran = true;
    });
    // Synchronous visibility: the body already ran before the returned promise was awaited.
    expect(ran).toBe(true);
    await promise;

    const skipSpy = vi.fn();
    const result = await withSavedDeckLibraryOrSkip(skipSpy);
    expect(result).toEqual({ status: "skipped", reason: "lock-unavailable" });
    expect(skipSpy).not.toHaveBeenCalled();

    // Paired positive: with the double installed, the same call commits.
    installFifoWebLocks();
    await resetSavedDeckLibraryForTests();
    const committed = await withSavedDeckLibraryOrSkip(() => "value");
    expect(committed).toEqual({ status: "committed", value: "value" });
  });

  it("a refused lock request runs the proceed policy's body once and the skip policy skips", async () => {
    Object.defineProperty(globalThis.navigator, "locks", {
      configurable: true,
      value: refusingWebLocks(new DOMException("nope", "InvalidStateError")),
    });
    const bodySpy = vi.fn(() => "value");
    await expect(withSavedDeckLibrary(bodySpy)).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);

    const result = await withSavedDeckLibraryOrSkip(vi.fn());
    expect(result).toEqual({ status: "skipped", reason: "lock-refused" });
  });

  it("rethrows a body error without retrying the body", async () => {
    const bodySpy = vi.fn(() => {
      throw new Error("boom");
    });
    await expect(withSavedDeckLibrary(bodySpy)).rejects.toThrow("boom");
    expect(bodySpy).toHaveBeenCalledTimes(1);
  });

  it("serializes two transactions FIFO with no overlap", async () => {
    const order: string[] = [];
    let releaseFirst!: () => void;
    const first = withSavedDeckLibrary(() => {
      order.push("first-start");
      return new Promise<void>((resolve) => {
        releaseFirst = resolve;
      });
    });
    const second = withSavedDeckLibrary(() => {
      order.push("second-start");
    });
    await vi.waitFor(() => expect(order).toEqual(["first-start"]));
    releaseFirst();
    await Promise.all([first, second]);
    expect(order).toEqual(["first-start", "second-start"]);
  });

  it("publishes the next generation to both IDB and localStorage after a committed transaction, and after a throw", async () => {
    await withSavedDeckLibrary(() => undefined);
    const idbGen1 = await readIdbGenerationForTests();
    const localGen1 = Number(localStorage.getItem("phase-saved-deck-library-generation"));
    expect(idbGen1).toBe(1);
    expect(localGen1).toBe(1);

    await expect(
      withSavedDeckLibrary(() => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    const idbGen2 = await readIdbGenerationForTests();
    const localGen2 = Number(localStorage.getItem("phase-saved-deck-library-generation"));
    expect(idbGen2).toBe(2);
    expect(localGen2).toBe(2);
  });

  it("IDB unavailable: the skip policy gives up before publishing and the proceed policy still runs the body", async () => {
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(() => {
      throw new Error("IDB unavailable");
    });
    const skipped = await withSavedDeckLibraryOrSkip(vi.fn());
    expect(skipped).toEqual({ status: "skipped", reason: "library-view-unconfirmed" });

    const bodySpy = vi.fn(() => "value");
    await expect(withSavedDeckLibrary(bodySpy)).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);
  });

  it("on a lock-wait timeout, the proceed policy runs the body unguarded and the skip policy skips", async () => {
    setSavedDeckTxnLockWaitForTests(null);
    vi.useFakeTimers();
    let releaseHolder!: () => void;
    const held = new Promise<void>((resolve) => {
      releaseHolder = resolve;
    });
    const holder = withSavedDeckLibrary(() => held);
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });

    const bodySpy = vi.fn(() => "value");
    const waiter = withSavedDeckLibrary(bodySpy);
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).pending).toHaveLength(1);
    });

    await vi.advanceTimersByTimeAsync(LOCK_WAIT_TIMEOUT_MS);
    await expect(waiter).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);
    expect((await navigator.locks.query()).held).toHaveLength(1); // holder still holds it
    expect((await navigator.locks.query()).pending).toHaveLength(0); // the aborted request left the queue

    const skipSpy = vi.fn();
    const skipWaiter = withSavedDeckLibraryOrSkip(skipSpy);
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).pending).toHaveLength(1);
    });
    await vi.advanceTimersByTimeAsync(LOCK_WAIT_TIMEOUT_MS);
    await expect(skipWaiter).resolves.toEqual({ status: "skipped", reason: "lock-timeout" });
    expect(skipSpy).not.toHaveBeenCalled();

    releaseHolder();
    await holder;
    vi.useRealTimers();
    setSavedDeckTxnLockWaitForTests(Number.POSITIVE_INFINITY);
  });

  it("the barrier waits for the local view to catch up to the committed IDB generation", async () => {
    await seedGenerationForTests(3, 2);
    const bodySpy = vi.fn(() => "value");
    const result = withSavedDeckLibraryOrSkip(bodySpy);
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });
    expect(bodySpy).not.toHaveBeenCalled();

    // Real delay past the barrier's own IDB read settling, not just past the lock grant that
    // preceded it: the body must still not have run.
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(bodySpy).not.toHaveBeenCalled();

    localStorage.setItem(GENERATION_KEY, "3");
    window.dispatchEvent(new StorageEvent("storage", { key: GENERATION_KEY }));

    await expect(result).resolves.toEqual({ status: "committed", value: "value" });
    expect(bodySpy).toHaveBeenCalledTimes(1);
  });

  it("the barrier's backstop poll wakes it when the local view catches up with no storage event", async () => {
    await seedGenerationForTests(3, 2);
    const bodySpy = vi.fn(() => "value");
    const result = withSavedDeckLibraryOrSkip(bodySpy);
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(bodySpy).not.toHaveBeenCalled();

    // A same-tab write never dispatches a native `storage` event; only the interval poll can
    // observe it.
    localStorage.setItem(GENERATION_KEY, "3");

    await expect(result).resolves.toEqual({ status: "committed", value: "value" });
    expect(bodySpy).toHaveBeenCalledTimes(1);
  });

  it("on a view-catch-up timeout, the skip policy skips and the proceed policy runs the body", async () => {
    await seedGenerationForTests(3, 2);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
    const skipSpy = vi.fn();
    const skipResult = withSavedDeckLibraryOrSkip(skipSpy);
    await vi.advanceTimersByTimeAsync(LIBRARY_VIEW_TIMEOUT_MS);
    await expect(skipResult).resolves.toEqual({ status: "skipped", reason: "library-view-unconfirmed" });
    expect(skipSpy).not.toHaveBeenCalled();

    const bodySpy = vi.fn(() => "value");
    const proceedResult = withSavedDeckLibrary(bodySpy);
    await vi.advanceTimersByTimeAsync(LIBRARY_VIEW_TIMEOUT_MS);
    await expect(proceedResult).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("an IDB get that hangs times out: the skip policy skips, the proceed policy proceeds after the second bound", async () => {
    // A hung transaction whose store's get/put never settle, on every mode.
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (this: IDBDatabase) {
      return {
        objectStore: () => ({ get: () => ({}) as IDBRequest, put: () => ({}) as IDBRequest }),
      } as unknown as IDBTransaction;
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
    const skipSpy = vi.fn();
    const skipResult = withSavedDeckLibraryOrSkip(skipSpy);
    await vi.advanceTimersByTimeAsync(GENERATION_IO_TIMEOUT_MS);
    await expect(skipResult).resolves.toEqual({ status: "skipped", reason: "library-view-unconfirmed" });
    expect(skipSpy).not.toHaveBeenCalled();

    const bodySpy = vi.fn(() => "value");
    const proceedResult = withSavedDeckLibrary(bodySpy);
    await vi.advanceTimersByTimeAsync(2 * GENERATION_IO_TIMEOUT_MS);
    await expect(proceedResult).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("an IDB set that hangs times out: the skip policy skips without running the body and still publishes locally; the proceed policy runs the body", async () => {
    // Readonly (get) resolves fast via a synthetic request (never touching real IDB open
    // machinery, which can outlast a single fake-timer advance); readwrite (set) hangs.
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (this: IDBDatabase, _storeNames: unknown, mode?: string) {
      if (mode === "readwrite") {
        return {
          objectStore: () => ({ get: () => ({}) as IDBRequest, put: () => ({}) as IDBRequest }),
        } as unknown as IDBTransaction;
      }
      const request = {} as IDBRequest & { result?: number };
      request.result = 0;
      queueMicrotask(() => request.onsuccess?.({} as Event));
      return { objectStore: () => ({ get: () => request }) } as unknown as IDBTransaction;
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
    const skipSpy = vi.fn();
    const skipResult = withSavedDeckLibraryOrSkip(skipSpy);
    await vi.advanceTimersByTimeAsync(GENERATION_IO_TIMEOUT_MS);
    await expect(skipResult).resolves.toEqual({ status: "skipped", reason: "generation-unpublished" });
    expect(skipSpy).not.toHaveBeenCalled();
    expect(Number(localStorage.getItem(GENERATION_KEY))).toBe(1);

    vi.restoreAllMocks();
    vi.useRealTimers();
    const bodySpy = vi.fn(() => "value");
    await expect(withSavedDeckLibrary(bodySpy)).resolves.toBe("value");
    expect(bodySpy).toHaveBeenCalledTimes(1);
  });

  it("write-ahead: on an IDB set rejection, the skip policy writes nothing and the proceed policy still writes", async () => {
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("", "QuotaExceededError");
    });
    let sentinelSkip = false;
    const skipResult = await withSavedDeckLibraryOrSkip(() => {
      sentinelSkip = true;
    });
    expect(skipResult).toEqual({ status: "skipped", reason: "generation-unpublished" });
    expect(sentinelSkip).toBe(false);

    let sentinelProceed = false;
    await withSavedDeckLibrary(() => {
      sentinelProceed = true;
    });
    expect(sentinelProceed).toBe(true);
  });

  it("a body sees its own generation already committed to IDB, one ahead of the localStorage view", async () => {
    await withSavedDeckLibrary(() => undefined); // previous generation = 1
    let idbSeen: number | undefined;
    let localSeen: string | null = null;
    await withSavedDeckLibrary(async () => {
      idbSeen = await readIdbGenerationForTests();
      localSeen = localStorage.getItem("phase-saved-deck-library-generation");
    });
    expect(idbSeen).toBe(2);
    expect(Number(localSeen ?? 0)).toBe(1);
  });

  it("the first transaction on a fresh library commits, and a later tab that lags behind the committed IDB generation waits for its local view before committing", async () => {
    expect(await readIdbGenerationForTests()).toBeUndefined();
    await withSavedDeckLibrary(() => undefined);
    expect(await readIdbGenerationForTests()).toBe(1);

    // Simulate tab B lagging: its localStorage mirror never received the first tab's publish
    // (removed here to stand in for propagation lag), even though IDB already holds generation 1.
    localStorage.removeItem(GENERATION_KEY);
    let bodyRan = false;
    const second = withSavedDeckLibraryOrSkip(() => {
      bodyRan = true;
    });
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(bodyRan).toBe(false);

    localStorage.setItem(GENERATION_KEY, "1");
    window.dispatchEvent(new StorageEvent("storage", { key: GENERATION_KEY }));
    await expect(second).resolves.toEqual({ status: "committed", value: undefined });
    expect(bodyRan).toBe(true);
  });
});
