/**
 * Autosave the deck a draft submission accepted into the saved-deck library,
 * under the ownership contract `constants/storage.ts::writeDraftAutosaveDeck`
 * enforces. Draft-specific: kind → slot, submission → `ParsedDeck`, and the
 * never-throw wrapper the stores call after a submission succeeds.
 */
import i18n from "i18next";
import type { DraftKind, DraftPlayerView } from "../adapter/draft-adapter";
import { CUSTOM_CUBE_SET_CODE } from "../adapter/draftKinds";
import { countProjectedNames } from "../components/draft/workspace/workspaceProjection";
import { writeDraftAutosaveDeck, type DraftAutosaveSlot } from "../constants/storage";
import type { ParsedDeck } from "./deckParser";
import { serializeSavedDeck } from "./savedDeckProjection";

/** A solo cube draft runs as kind `Quick`; only its set code tells it apart. */
export function draftAutosaveSlot(kind: DraftKind, setCode: string | null): DraftAutosaveSlot {
  switch (kind) {
    case "Quick":
      return setCode === CUSTOM_CUBE_SET_CODE ? "Cube" : "Quick";
    case "Premier":
      return "Premier";
    case "Traditional":
      return "Traditional";
    case "Sealed":
      return "Sealed";
    case "CommanderDraft":
      return "CommanderDraft";
    case "Winston":
      return "Winston";
  }
}

function autosaveSlotLabels(): Record<DraftAutosaveSlot, string> {
  return {
    Quick: i18n.t("draft:deckAutosave.slotQuick"),
    Sealed: i18n.t("draft:deckAutosave.slotSealed"),
    Cube: i18n.t("draft:deckAutosave.slotCube"),
    Premier: i18n.t("draft:deckAutosave.slotPremier"),
    Traditional: i18n.t("draft:deckAutosave.slotTraditional"),
    CommanderDraft: i18n.t("draft:deckAutosave.slotCommanderDraft"),
    Winston: i18n.t("draft:deckAutosave.slotWinston"),
  };
}

/** The saved-deck form of an accepted draft submission. Saved decks keep commanders out of `main`; the
 *  sideboard is the rest of the pool. */
export function draftSubmissionToParsedDeck(
  mainDeck: readonly string[],
  commanders: readonly string[],
  pool: readonly { name: string }[],
): ParsedDeck {
  const mainNames = [...mainDeck];
  for (const commander of commanders) {
    const index = mainNames.indexOf(commander);
    if (index !== -1) mainNames.splice(index, 1);
  }

  const remainingPool = pool.map((card) => card.name);
  for (const name of mainDeck) {
    const index = remainingPool.indexOf(name);
    if (index !== -1) remainingPool.splice(index, 1);
  }

  return {
    main: countProjectedNames(mainNames),
    sideboard: countProjectedNames(remainingPool),
    commander: commanders.length > 0 ? [...commanders] : undefined,
  };
}

export interface DraftDeckAutosave {
  view: Pick<DraftPlayerView, "kind" | "pool" | "commanders_required">;
  setCode: string | null;
  mainDeck: readonly string[];
  commanders: readonly string[];
}

/** Never throws: a failed autosave must not fail the deck submission that triggered it. */
export function autosaveDraftDeck(submission: DraftDeckAutosave): void {
  try {
    const slot = draftAutosaveSlot(submission.view.kind, submission.setCode);
    const label = i18n.t("draft:deckAutosave.deckName", { format: autosaveSlotLabels()[slot] });
    const format = submission.view.commanders_required > 0 ? "CommanderDraft" : "Limited";
    const deck = draftSubmissionToParsedDeck(submission.mainDeck, submission.commanders, submission.view.pool);
    writeDraftAutosaveDeck(slot, label, serializeSavedDeck(deck, format, null));
  } catch (error) {
    console.warn("[draftDeckAutosave] autosave failed:", error);
  }
}
