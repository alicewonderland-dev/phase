import { clearDeckAutosaveMarker, STORAGE_KEY_PREFIX, writeSavedDeckData } from "../constants/storage";
import type { DeckEntry } from "../hooks/useDecks";
import type { ParsedDeck } from "./deckParser";
import { withSavedDeckLibrary } from "./savedDeckTransaction";

export function preconDeckEntryToParsedDeck(deck: DeckEntry): ParsedDeck {
  return {
    main: deck.mainBoard.map((c) => ({ name: c.name, count: c.count })),
    sideboard: (deck.sideBoard ?? []).map((c) => ({ name: c.name, count: c.count })),
    commander:
      deck.commander && deck.commander.length > 0
        ? deck.commander.map((c) => c.name)
        : undefined,
  };
}

export function preconExists(savedName: string): boolean {
  return localStorage.getItem(STORAGE_KEY_PREFIX + savedName) !== null;
}

/** Whether a name that already exists in the library should be replaced or left as-is. */
export type ExistingDeckPolicy = "replace" | "keep";

/**
 * Persist a preconstructed deck under the user's saved-decks namespace so it
 * participates in the normal deck-compatibility / active-deck / tile-render
 * flows without any precon-specific branching downstream. When `onExisting`
 * is `"keep"`, a deck already saved under `savedName` is left untouched.
 */
export function savePreconDeck(
  savedName: string,
  deck: DeckEntry,
  onExisting: ExistingDeckPolicy,
): Promise<"saved" | "kept-existing"> {
  const parsed = preconDeckEntryToParsedDeck(deck);
  return withSavedDeckLibrary((txn) => {
    if (onExisting === "keep" && localStorage.getItem(STORAGE_KEY_PREFIX + savedName) !== null) {
      return "kept-existing";
    }
    writeSavedDeckData(txn, savedName, JSON.stringify(parsed));
    clearDeckAutosaveMarker(txn, savedName);
    return "saved";
  });
}
