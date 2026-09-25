import { beforeEach, describe, expect, it } from "vitest";

import { savePreconDeck } from "../preconDecks";
import type { DeckEntry } from "../../hooks/useDecks";
import { getDeckMeta, STORAGE_KEY_PREFIX, writeDraftAutosaveDeck } from "../../constants/storage";

beforeEach(() => {
  localStorage.clear();
});

describe("savePreconDeck", () => {
  it("clears an autosave marker on the deck it overwrites", () => {
    const name = writeDraftAutosaveDeck("Sealed", "[Autosave] Sealed", "autosave-data");
    const precon: DeckEntry = {
      name: "Precon", code: "SET", type: "Commander Deck", coveragePct: 100,
      mainBoard: [{ name: "Forest", count: 40 }],
      sideBoard: [],
      commander: undefined,
    };

    savePreconDeck(name, precon);

    expect(getDeckMeta(name)?.autosaveSlot).toBeUndefined();
    const persisted = JSON.parse(localStorage.getItem(STORAGE_KEY_PREFIX + name) ?? "{}");
    expect(persisted.main).toEqual([{ name: "Forest", count: 40 }]);
  });
});
