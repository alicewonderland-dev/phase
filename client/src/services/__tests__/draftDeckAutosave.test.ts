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
  it("removes one main-deck copy per commander and sideboards the rest of the pool", () => {
    const mainDeck = ["Commander Card", "Commander Card", "Bolt", "Plains"];
    const commanders = ["Commander Card"];
    const pool = [
      { name: "Commander Card" }, { name: "Commander Card" },
      { name: "Bolt" }, { name: "Bear" },
    ];

    const deck = draftSubmissionToParsedDeck(mainDeck, commanders, pool);

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
    const deck = draftSubmissionToParsedDeck(["Bolt"], [], [{ name: "Bolt" }]);
    expect(deck.commander).toBeUndefined();
    expect(deck.sideboard).toEqual([]);
  });

  it("subtracts nothing from the pool for a virtual basic land in the main deck", () => {
    const deck = draftSubmissionToParsedDeck(
      ["Bolt", "Island"], [], [{ name: "Bolt" }, { name: "Bear" }],
    );
    expect(deck.main).toEqual(expect.arrayContaining([
      { name: "Bolt", count: 1 }, { name: "Island", count: 1 },
    ]));
    expect(deck.sideboard).toEqual([{ name: "Bear", count: 1 }]);
  });
});
