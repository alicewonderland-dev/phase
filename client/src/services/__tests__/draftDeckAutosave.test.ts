import { describe, expect, it } from "vitest";

import { CUSTOM_CUBE_SET_CODE, DRAFT_KINDS } from "../../adapter/draftKinds";
import { draftAutosaveSlot, draftSubmissionToParsedDeck } from "../draftDeckAutosave";

describe("draftAutosaveSlot", () => {
  it("distinguishes a solo cube draft (kind Quick, custom-cube set code) from a solo Quick draft", () => {
    expect(draftAutosaveSlot("Quick", "TST")).toBe("Quick");
    expect(draftAutosaveSlot("Quick", CUSTOM_CUBE_SET_CODE)).toBe("Cube");
    expect(draftAutosaveSlot("Quick", null)).toBe("Quick");
  });

  it.each(DRAFT_KINDS.filter((kind) => kind !== "Quick"))(
    "maps every other draft kind (%s) to its own slot, regardless of set code",
    (kind) => {
      expect(draftAutosaveSlot(kind, "TST")).toBe(kind);
      expect(draftAutosaveSlot(kind, CUSTOM_CUBE_SET_CODE)).toBe(kind);
    },
  );
});

describe("draftSubmissionToParsedDeck", () => {
  it("removes one main-deck copy per commander and saves the partition's sideboard", () => {
    const partition = {
      mainDeck: ["Commander Card", "Commander Card", "Bolt", "Plains"],
      sideboard: ["Bear"],
    };
    const commanders = ["Commander Card"];

    const deck = draftSubmissionToParsedDeck(partition, commanders);

    expect(deck.main).toEqual(expect.arrayContaining([
      { name: "Commander Card", count: 1 },
      { name: "Bolt", count: 1 },
      { name: "Plains", count: 1 },
    ]));
    expect(deck.main).toHaveLength(3);
    expect(deck.sideboard).toEqual([{ name: "Bear", count: 1 }]);
    expect(deck.commander).toEqual(["Commander Card"]);
  });

  it("omits the commander field for a non-commander submission", () => {
    const deck = draftSubmissionToParsedDeck({ mainDeck: ["Bolt"], sideboard: [] }, []);
    expect(deck.commander).toBeUndefined();
    expect(deck.sideboard).toEqual([]);
  });
});
