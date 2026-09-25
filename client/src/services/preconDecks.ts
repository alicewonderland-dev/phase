import { clearDeckAutosaveMarker, STORAGE_KEY_PREFIX } from "../constants/storage";
import type { DeckEntry } from "../hooks/useDecks";
import type { ParsedDeck } from "./deckParser";

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

/**
 * Persist a preconstructed deck under the user's saved-decks namespace so it
 * participates in the normal deck-compatibility / active-deck / tile-render
 * flows without any precon-specific branching downstream.
 */
export function savePreconDeck(savedName: string, deck: DeckEntry): void {
  const parsed = preconDeckEntryToParsedDeck(deck);
  localStorage.setItem(STORAGE_KEY_PREFIX + savedName, JSON.stringify(parsed));
  clearDeckAutosaveMarker(savedName);
}
