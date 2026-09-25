import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  ACTIVE_DECK_KEY,
  createFolder,
  deleteFolder,
  DRAFT_WORKSPACE_PREFERENCES_KEY,
  getDeckMeta,
  isUserOwnedStorageKey,
  listFolders,
  listSavedDeckNames,
  loadSavedDeck,
  loadSavedDeckBracket,
  loadSavedDeckFormat,
  migrateDeckMeta,
  renameFolder,
  saveSavedDeckBracket,
  setDeckFolder,
  stampDeckMeta,
  toggleDeckStar,
  touchDeckPlayed,
  uniqueDeckName,
  writeDraftAutosaveDeck,
  STORAGE_KEY_PREFIX,
} from "../storage";
import { expandParsedDeck } from "../../services/deckParser";
import {
  installFifoWebLocks,
  resetSavedDeckLibraryForTests,
  testSavedDeckTxn,
  uninstallWebLocks,
} from "../../test/helpers/webLocks";
import type { SavedDeckTxnResult } from "../../services/savedDeckTransaction";

beforeEach(() => {
  localStorage.clear();
});

describe("user-owned storage keys", () => {
  it("owns only the exact draft workspace preference key", () => {
    expect(DRAFT_WORKSPACE_PREFERENCES_KEY).toBe("phase-draft-workspace-preferences");
    expect(isUserOwnedStorageKey(DRAFT_WORKSPACE_PREFERENCES_KEY)).toBe(true);
    expect(isUserOwnedStorageKey(`${DRAFT_WORKSPACE_PREFERENCES_KEY}-copy`)).toBe(false);
    expect(isUserOwnedStorageKey(`copy-${DRAFT_WORKSPACE_PREFERENCES_KEY}`)).toBe(false);
    expect(isUserOwnedStorageKey(DRAFT_WORKSPACE_PREFERENCES_KEY.toUpperCase())).toBe(false);
  });
});

describe("saved-deck bracket sidecar", () => {
  it("reads the persisted deck format without projecting deck data", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Oathbreaker Deck",
      JSON.stringify({ main: [], sideboard: [], format: "Oathbreaker" }),
    );

    expect(loadSavedDeckFormat("Oathbreaker Deck")).toBe("Oathbreaker");
    expect(loadSavedDeckFormat("Missing Deck")).toBeUndefined();
  });

  it.each(["Commander", "Brawl"] as const)(
    "keeps a dedicated companion and removes one stale sideboard copy for %s reads",
    (format) => {
      const raw = JSON.stringify({
        main: [{ count: 1, name: "Sol Ring" }],
        sideboard: [{ count: 2, name: "Lurrus of the Dream-Den" }],
        commander: ["Alela, Artful Provocateur"],
        companion: "Lurrus of the Dream-Den",
        format,
      });
      localStorage.setItem(STORAGE_KEY_PREFIX + "Legacy Commander", raw);

      const loaded = loadSavedDeck("Legacy Commander");

      expect(loaded?.companion).toBe("Lurrus of the Dream-Den");
      expect(loaded?.sideboard).toEqual([{ count: 1, name: "Lurrus of the Dream-Den" }]);
      expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Legacy Commander")).toBe(raw);
    },
  );

  it("materializes a traditional companion in the sideboard and clears its dedicated slot", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Legacy Modern",
      JSON.stringify({
        main: [{ count: 1, name: "Sol Ring" }],
        sideboard: [],
        companion: "Lurrus of the Dream-Den",
        format: "Modern",
      }),
    );

    const loaded = loadSavedDeck("Legacy Modern");

    expect(loaded?.companion).toBeUndefined();
    expect(loaded?.sideboard).toEqual([{ count: 1, name: "Lurrus of the Dream-Den" }]);
  });

  it("keeps signature spells only for Oathbreaker saved-deck reads", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Modern Signature",
      JSON.stringify({
        main: [{ count: 1, name: "Lightning Bolt" }],
        sideboard: [],
        signature_spell: ["Lightning Bolt"],
        format: "Modern",
      }),
    );
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Oathbreaker Signature",
      JSON.stringify({
        main: [{ count: 1, name: "Lightning Bolt" }],
        sideboard: [],
        signature_spell: ["Lightning Bolt"],
        format: "Oathbreaker",
      }),
    );

    expect(loadSavedDeck("Modern Signature")?.signature_spell).toBeUndefined();
    expect(loadSavedDeck("Oathbreaker Signature")?.signature_spell).toEqual(["Lightning Bolt"]);
  });

  it("preserves sticker sheets when loading and expanding a saved deck", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Sticker Deck",
      JSON.stringify({
        main: [{ count: 1, name: "Sol Ring" }],
        sideboard: [],
        sticker_sheets: ["sheet-1", "sheet-2", "sheet-3"],
      }),
    );

    const loaded = loadSavedDeck("Sticker Deck");

    expect(loaded?.sticker_sheets).toEqual(["sheet-1", "sheet-2", "sheet-3"]);
    expect(loaded && expandParsedDeck(loaded).sticker_sheets).toEqual(["sheet-1", "sheet-2", "sheet-3"]);
  });

  it("preserves planar decks when loading and expanding a saved deck", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Planar Deck",
      JSON.stringify({
        main: [{ count: 1, name: "Sol Ring" }],
        sideboard: [],
        planar_deck: ["The Aether Flues", "Spatial Merging"],
      }),
    );

    const loaded = loadSavedDeck("Planar Deck");

    expect(loaded?.planar_deck).toEqual(["The Aether Flues", "Spatial Merging"]);
    expect(loaded && expandParsedDeck(loaded).planar_deck).toEqual(["The Aether Flues", "Spatial Merging"]);
  });

  it("returns null when the deck does not exist", () => {
    expect(loadSavedDeckBracket("Missing Deck")).toBeNull();
  });

  it("returns null when the persisted JSON has no bracket field", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Untagged",
      JSON.stringify({ main: [], sideboard: [], format: "Commander" }),
    );
    expect(loadSavedDeckBracket("Untagged")).toBeNull();
  });

  it("returns the bracket when persisted", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Tagged",
      JSON.stringify({ main: [], sideboard: [], format: "Commander", bracket: 3 }),
    );
    expect(loadSavedDeckBracket("Tagged")).toBe(3);
  });

  it("returns null when the persisted bracket is invalid (e.g. 0 or 'x')", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Bad",
      JSON.stringify({ main: [], sideboard: [], format: "Commander", bracket: 0 }),
    );
    expect(loadSavedDeckBracket("Bad")).toBeNull();
  });

  it("saveSavedDeckBracket merges the bracket into the existing persisted JSON", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Existing",
      JSON.stringify({ main: [{ count: 1, name: "Sol Ring" }], sideboard: [], format: "Commander" }),
    );
    saveSavedDeckBracket(testSavedDeckTxn, "Existing", 4);
    const raw = localStorage.getItem(STORAGE_KEY_PREFIX + "Existing")!;
    const parsed = JSON.parse(raw);
    expect(parsed.bracket).toBe(4);
    // Pre-existing fields must be preserved.
    expect(parsed.main).toEqual([{ count: 1, name: "Sol Ring" }]);
    expect(parsed.format).toBe("Commander");
  });

  it("saveSavedDeckBracket with null removes any existing bracket field", () => {
    localStorage.setItem(
      STORAGE_KEY_PREFIX + "Existing",
      JSON.stringify({ main: [], sideboard: [], format: "Commander", bracket: 4 }),
    );
    saveSavedDeckBracket(testSavedDeckTxn, "Existing", null);
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY_PREFIX + "Existing")!);
    expect("bracket" in parsed).toBe(false);
  });

  it("saveSavedDeckBracket is a no-op when the deck does not exist", () => {
    saveSavedDeckBracket(testSavedDeckTxn, "Missing", 3);
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Missing")).toBeNull();
  });
});

describe("folder registry", () => {
  it("createFolder appends with an incrementing order and returns the folder", () => {
    const a = createFolder("Control");
    const b = createFolder("Aggro");
    expect(a).not.toBeNull();
    expect(a?.name).toBe("Control");
    expect(a?.order).toBe(0);
    expect(b?.order).toBe(1);
    expect(a?.id).not.toBe(b?.id);
    expect(listFolders().map((f) => f.name)).toEqual(["Control", "Aggro"]);
  });

  it("createFolder trims, caps length, and rejects blank names", () => {
    expect(createFolder("   ")).toBeNull();
    const folder = createFolder(`  ${"x".repeat(60)}  `);
    expect(folder?.name).toHaveLength(40);
  });

  it("listFolders sorts by order then name", () => {
    createFolder("Zed"); // order 0
    createFolder("Alpha"); // order 1
    // Same order value sorts by name as a tiebreak.
    localStorage.setItem(
      "phase-deck-folders",
      JSON.stringify([
        { id: "1", name: "Zed", order: 5 },
        { id: "2", name: "Alpha", order: 5 },
      ]),
    );
    expect(listFolders().map((f) => f.name)).toEqual(["Alpha", "Zed"]);
  });

  it("renameFolder updates the name and ignores unknown ids / blanks", () => {
    const folder = createFolder("Old")!;
    renameFolder(folder.id, "New");
    expect(listFolders()[0].name).toBe("New");
    renameFolder(folder.id, "  ");
    expect(listFolders()[0].name).toBe("New");
    renameFolder("nonexistent", "Ghost");
    expect(listFolders()).toHaveLength(1);
  });

  it("deleteFolder removes the folder and reassigns its decks to Unfiled", () => {
    const folder = createFolder("Brews")!;
    stampDeckMeta(testSavedDeckTxn, "Deck A");
    setDeckFolder(testSavedDeckTxn, "Deck A", folder.id);
    expect(getDeckMeta("Deck A")?.folderId).toBe(folder.id);

    deleteFolder(testSavedDeckTxn, folder.id);

    expect(listFolders()).toHaveLength(0);
    // Deck survives; only its folder membership is cleared.
    expect(getDeckMeta("Deck A")?.folderId).toBeUndefined();
  });
});

describe("deck membership + stars", () => {
  it("setDeckFolder assigns and clears membership", () => {
    const folder = createFolder("Commander")!;
    stampDeckMeta(testSavedDeckTxn, "Atraxa");
    setDeckFolder(testSavedDeckTxn, "Atraxa", folder.id);
    expect(getDeckMeta("Atraxa")?.folderId).toBe(folder.id);
    setDeckFolder(testSavedDeckTxn, "Atraxa", null);
    expect(getDeckMeta("Atraxa")?.folderId).toBeUndefined();
  });

  it("setDeckFolder seeds metadata for a deck that was never stamped", () => {
    const folder = createFolder("Imported")!;
    setDeckFolder(testSavedDeckTxn, "Fresh Import", folder.id);
    const meta = getDeckMeta("Fresh Import");
    expect(meta?.folderId).toBe(folder.id);
    expect(typeof meta?.addedAt).toBe("number");
  });

  it("toggleDeckStar flips and returns the resulting state", () => {
    stampDeckMeta(testSavedDeckTxn, "Burn");
    expect(toggleDeckStar(testSavedDeckTxn, "Burn")).toBe(true);
    expect(getDeckMeta("Burn")?.starred).toBe(true);
    expect(toggleDeckStar(testSavedDeckTxn, "Burn")).toBe(false);
    expect(getDeckMeta("Burn")?.starred).toBeUndefined();
  });
});

describe("metadata migration on rename", () => {
  it("migrateDeckMeta carries folder, star, and timestamps to the new name", () => {
    const folder = createFolder("Modern")!;
    stampDeckMeta(testSavedDeckTxn, "Old Name", 1000);
    setDeckFolder(testSavedDeckTxn, "Old Name", folder.id);
    toggleDeckStar(testSavedDeckTxn, "Old Name");
    touchDeckPlayed(testSavedDeckTxn, "Old Name");
    const before = getDeckMeta("Old Name")!;

    migrateDeckMeta(testSavedDeckTxn, "Old Name", "New Name");

    expect(getDeckMeta("Old Name")).toBeNull();
    const after = getDeckMeta("New Name")!;
    expect(after.folderId).toBe(folder.id);
    expect(after.starred).toBe(true);
    expect(after.addedAt).toBe(1000);
    expect(after.lastPlayedAt).toBe(before.lastPlayedAt);
  });

  it("migrateDeckMeta is a no-op when the source has no metadata", () => {
    migrateDeckMeta(testSavedDeckTxn, "Never Stamped", "New Name");
    expect(getDeckMeta("New Name")).toBeNull();
  });

  it("migrateDeckMeta is a no-op when source and target names match", () => {
    stampDeckMeta(testSavedDeckTxn, "Same", 500);
    migrateDeckMeta(testSavedDeckTxn, "Same", "Same");
    expect(getDeckMeta("Same")?.addedAt).toBe(500);
  });
});

describe("touchDeckPlayed preserves organization", () => {
  it("keeps folderId and starred when stamping lastPlayedAt", () => {
    const folder = createFolder("Pauper")!;
    stampDeckMeta(testSavedDeckTxn, "Affinity");
    setDeckFolder(testSavedDeckTxn, "Affinity", folder.id);
    toggleDeckStar(testSavedDeckTxn, "Affinity");

    touchDeckPlayed(testSavedDeckTxn, "Affinity");

    const meta = getDeckMeta("Affinity")!;
    expect(meta.folderId).toBe(folder.id);
    expect(meta.starred).toBe(true);
    expect(typeof meta.lastPlayedAt).toBe("number");
  });
});

describe("uniqueDeckName", () => {
  it("keeps the import suffix and accepts a custom candidate", () => {
    expect(uniqueDeckName("X", ["X"])).toBe("X 2");
    expect(uniqueDeckName("X", ["X"], (i) => `X (${i})`)).toBe("X (2)");
    expect(uniqueDeckName("Fresh", [])).toBe("Fresh");
  });
});

describe("draft autosave ownership", () => {
  beforeEach(async () => {
    installFifoWebLocks();
    await resetSavedDeckLibraryForTests();
  });
  afterEach(() => {
    uninstallWebLocks();
  });

  /** Unwrap a committed result, failing the test on a skip instead of returning `undefined`. */
  function committedName(result: SavedDeckTxnResult<string>): string {
    if (result.status === "skipped") throw new Error(`writeDraftAutosaveDeck skipped: ${result.reason}`);
    return result.value;
  }

  it("with navigator.locks null, writeDraftAutosaveDeck resolves lock-unavailable and writes nothing", async () => {
    uninstallWebLocks();
    const result = await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "deck-data-1");
    expect(result).toEqual({ status: "skipped", reason: "lock-unavailable" });
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed")).toBeNull();
  });

  it("creates the slot deck on the first autosave", async () => {
    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "deck-data-1"));
    expect(name).toBe("[Autosave] Sealed");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + name)).toBe("deck-data-1");
    expect(getDeckMeta(name)?.autosaveSlot).toBe("Sealed");
  });

  it("never overwrites an unmarked deck occupying the label", async () => {
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed", "user-deck");
    stampDeckMeta(testSavedDeckTxn, "[Autosave] Sealed");

    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "autosave-data"));

    expect(name).toBe("[Autosave] Sealed (2)");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed")).toBe("user-deck");
    expect(getDeckMeta("[Autosave] Sealed")?.autosaveSlot).toBeUndefined();
    expect(getDeckMeta(name)?.autosaveSlot).toBe("Sealed");
  });

  it("skips every taken suffix", async () => {
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed", "user-deck");
    stampDeckMeta(testSavedDeckTxn, "[Autosave] Sealed");
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed (2)", "user-deck-2");
    stampDeckMeta(testSavedDeckTxn, "[Autosave] Sealed (2)");

    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "autosave-data"));

    expect(name).toBe("[Autosave] Sealed (3)");
  });

  it("overwrites the marked deck in place on a repeat autosave", async () => {
    const first = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-1"));
    const second = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-2"));

    expect(second).toBe(first);
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + first)).toBe("data-2");
    expect(getDeckMeta(first)?.autosaveSlot).toBe("Sealed");
    expect(listSavedDeckNames()).toEqual([first]);
  });

  it("moves a marked deck to the freed label, carrying folder, star, and the active pointer", async () => {
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed (2)", "data-1");
    const folder = createFolder("Drafts")!;
    const store = JSON.parse(localStorage.getItem("phase-deck-metadata") ?? "{}");
    store["[Autosave] Sealed (2)"] = { addedAt: 1, autosaveSlot: "Sealed", folderId: folder.id, starred: true };
    localStorage.setItem("phase-deck-metadata", JSON.stringify(store));
    localStorage.setItem(ACTIVE_DECK_KEY, "[Autosave] Sealed (2)");

    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-2"));

    expect(name).toBe("[Autosave] Sealed");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed (2)")).toBeNull();
    const meta = getDeckMeta(name)!;
    expect(meta.folderId).toBe(folder.id);
    expect(meta.starred).toBe(true);
    expect(meta.autosaveSlot).toBe("Sealed");
    expect(localStorage.getItem(ACTIVE_DECK_KEY)).toBe(name);
  });

  it("keeps a single owner per slot, clearing the marker on the other", async () => {
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed", "data-a");
    stampDeckMeta(testSavedDeckTxn, "[Autosave] Sealed");
    localStorage.setItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed", "");
    // Mark two decks with the same slot directly, bypassing the writer.
    const store = JSON.parse(localStorage.getItem("phase-deck-metadata") ?? "{}");
    store["[Autosave] Sealed"] = { addedAt: 1, autosaveSlot: "Sealed" };
    store["Other Sealed Deck"] = { addedAt: 2, autosaveSlot: "Sealed" };
    localStorage.setItem("phase-deck-metadata", JSON.stringify(store));
    localStorage.setItem(STORAGE_KEY_PREFIX + "Other Sealed Deck", "data-b");

    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-new"));

    expect(name).toBe("[Autosave] Sealed");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Other Sealed Deck")).toBe("data-b");
    expect(getDeckMeta("Other Sealed Deck")?.autosaveSlot).toBeUndefined();
    expect(getDeckMeta(name)?.autosaveSlot).toBe("Sealed");
  });

  it("does not treat an orphaned marker (no deck key) as an owner", async () => {
    const store = JSON.parse(localStorage.getItem("phase-deck-metadata") ?? "{}");
    store["[Autosave] Sealed"] = { addedAt: 1, autosaveSlot: "Sealed", folderId: "stale-folder" };
    localStorage.setItem("phase-deck-metadata", JSON.stringify(store));

    const name = committedName(await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "fresh-data"));

    expect(name).toBe("[Autosave] Sealed");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + name)).toBe("fresh-data");
    expect(getDeckMeta(name)?.folderId).toBeUndefined();
    expect(getDeckMeta(name)?.autosaveSlot).toBe("Sealed");
  });

  it("stamping a marked deck clears only the marker", async () => {
    await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-1");
    const folder = createFolder("Kept")!;
    setDeckFolder(testSavedDeckTxn, "[Autosave] Sealed", folder.id);

    stampDeckMeta(testSavedDeckTxn, "[Autosave] Sealed");

    const meta = getDeckMeta("[Autosave] Sealed")!;
    expect(meta.autosaveSlot).toBeUndefined();
    expect(meta.folderId).toBe(folder.id);
  });

  it("play, folder, and star mutations keep the marker", async () => {
    await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "data-1");
    const folder = createFolder("Kept")!;

    touchDeckPlayed(testSavedDeckTxn, "[Autosave] Sealed");
    setDeckFolder(testSavedDeckTxn, "[Autosave] Sealed", folder.id);
    toggleDeckStar(testSavedDeckTxn, "[Autosave] Sealed");

    expect(getDeckMeta("[Autosave] Sealed")?.autosaveSlot).toBe("Sealed");
  });

  it("does not cross autosave slots", async () => {
    await writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "sealed-data");
    await writeDraftAutosaveDeck("Quick", "[Autosave] Quick Draft", "quick-data");

    expect(getDeckMeta("[Autosave] Sealed")?.autosaveSlot).toBe("Sealed");
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "[Autosave] Sealed")).toBe("sealed-data");
  });
});
