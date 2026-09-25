import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";

import type { DeckEntry } from "../../hooks/useDecks";
import { savePreconDeck } from "../preconDecks";
import { adoptFeedDeck, unsubscribe } from "../feedService";
import { importBackupFromFile, type PhaseBackupV1 } from "../backup";
import { saveBuilderDeck } from "../../constants/storage";
import { useDeckFolders } from "../../hooks/useDeckFolders";
import { withSavedDeckLibrary } from "../savedDeckTransaction";
import {
  installFifoWebLocks,
  resetSavedDeckLibraryForTests,
  uninstallWebLocks,
} from "../../test/helpers/webLocks";
import { STORAGE_KEY_PREFIX, DECK_METADATA_KEY, FEED_DECK_ORIGINS_KEY } from "../../constants/storage";

/**
 * Each row holds the saved-deck library lock with a deferred body, invokes the writer, waits
 * for the writer's own request to reach the queue (the reach guard proving it took the lock),
 * asserts storage is unchanged while it waits, then releases and asserts the write landed.
 */

beforeEach(async () => {
  localStorage.clear();
  installFifoWebLocks();
  await resetSavedDeckLibraryForTests();
});

afterEach(() => {
  uninstallWebLocks();
});

async function heldLock(): Promise<{ release: () => void; holder: Promise<void> }> {
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const holder = withSavedDeckLibrary(() => held);
  await vi.waitFor(async () => {
    expect((await navigator.locks.query()).held).toHaveLength(1);
  });
  return { release, holder };
}

async function waitPendingThenRelease(release: () => void, holder: Promise<void>): Promise<void> {
  await vi.waitFor(async () => {
    expect((await navigator.locks.query()).pending).toHaveLength(1);
  });
  release();
  await holder;
  await vi.waitFor(async () => {
    expect((await navigator.locks.query()).held).toHaveLength(0);
    expect((await navigator.locks.query()).pending).toHaveLength(0);
  });
}

describe("savedDeckWriters: each takes the saved-deck library lock", () => {
  it("savePreconDeck waits for the lock and then writes", async () => {
    const { release, holder } = await heldLock();
    const precon: DeckEntry = {
      name: "Precon",
      code: "SET",
      type: "Commander Deck",
      coveragePct: 100,
      mainBoard: [{ name: "Forest", count: 40 }],
      sideBoard: [],
      commander: undefined,
    };
    const call = savePreconDeck("Precon Deck", precon);
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Precon Deck")).toBeNull();
    await waitPendingThenRelease(release, holder);
    await call;
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Precon Deck")).not.toBeNull();
  });

  it("adoptFeedDeck waits for the lock and then writes", async () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Feed Deck",
      JSON.stringify({ main: [{ name: "Bear", count: 1 }], sideboard: [] }),
    );
    const { release, holder } = await heldLock();
    const call = adoptFeedDeck("Feed Deck", "Adopted Deck");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Adopted Deck")).toBeNull();
    await waitPendingThenRelease(release, holder);
    await call;
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Adopted Deck")).not.toBeNull();
  });

  it("unsubscribe waits for the lock and then removes feed-owned decks", async () => {
    localStorage.setItem(
      FEED_DECK_ORIGINS_KEY,
      JSON.stringify({ "Feed Deck": "feed-1" }),
    );
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Feed Deck",
      JSON.stringify({ main: [], sideboard: [] }),
    );
    const { release, holder } = await heldLock();
    const call = unsubscribe("feed-1");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Feed Deck")).not.toBeNull();
    await waitPendingThenRelease(release, holder);
    await call;
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Feed Deck")).toBeNull();
  });

  it("importBackupFromFile waits for the lock and then applies the backup", async () => {
    const backup: PhaseBackupV1 = {
      version: 1,
      exportedAt: new Date(0).toISOString(),
      preferences: null,
      decks: { "Imported Deck": JSON.stringify({ main: [], sideboard: [] }) },
      deckMetadata: null,
      activeDeck: null,
      feedSubscriptions: null,
      feedDeckOrigins: null,
    };
    const file = new File([JSON.stringify(backup)], "phase-backup.json", { type: "application/json" });
    const { release, holder } = await heldLock();
    const call = importBackupFromFile(file, "merge");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Imported Deck")).toBeNull();
    await waitPendingThenRelease(release, holder);
    const result = await call;
    expect(result.decksImported).toBe(1);
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Imported Deck")).not.toBeNull();
  });

  it("saveBuilderDeck waits for the lock and then writes", async () => {
    const { release, holder } = await heldLock();
    const call = saveBuilderDeck(null, "Built Deck", JSON.stringify({ main: [], sideboard: [] }));
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Built Deck")).toBeNull();
    await waitPendingThenRelease(release, holder);
    await call;
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Built Deck")).not.toBeNull();
  });

  it("useDeckFolders().toggleStar waits for the lock and then flips the star", async () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Starrable Deck",
      JSON.stringify({ main: [], sideboard: [] }),
    );
    const { result } = renderHook(() => useDeckFolders());
    const { release, holder } = await heldLock();
    let call!: Promise<boolean>;
    act(() => {
      call = result.current.toggleStar("Starrable Deck");
    });
    expect(JSON.parse(localStorage.getItem(DECK_METADATA_KEY) ?? "{}")["Starrable Deck"]?.starred).toBeFalsy();
    await waitPendingThenRelease(release, holder);
    await act(async () => {
      await call;
    });
    expect(JSON.parse(localStorage.getItem(DECK_METADATA_KEY) ?? "{}")["Starrable Deck"]?.starred).toBe(true);
  });
});
