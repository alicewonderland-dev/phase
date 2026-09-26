import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { PreconDeckModal } from "../PreconDeckModal";
import { STORAGE_KEY_PREFIX, writeSavedDeckData } from "../../../constants/storage";
import type { DeckMap } from "../../../hooks/useDecks";
import { withSavedDeckLibrary } from "../../../services/savedDeckTransaction";
import {
  installFifoWebLocks,
  resetSavedDeckLibraryForTests,
  uninstallWebLocks,
} from "../../../test/helpers/webLocks";

const decks: DeckMap = {
  aggro: {
    code: "SET",
    name: "Aggro Deck",
    type: "Commander Deck",
    releaseDate: "2026-01-01",
    coveragePct: 100,
    mainBoard: [{ name: "Mountain", count: 40 }],
    sideBoard: [],
    commander: [{ name: "Krenko", count: 1 }],
  },
  control: {
    code: "SET",
    name: "Control Deck",
    type: "Commander Deck",
    releaseDate: "2026-01-02",
    coveragePct: 100,
    mainBoard: [{ name: "Island", count: 40 }],
    sideBoard: [],
    commander: [{ name: "Teferi", count: 1 }],
  },
};

vi.mock("../../../hooks/useDecks", async () => {
  const actual = await vi.importActual<typeof import("../../../hooks/useDecks")>("../../../hooks/useDecks");
  return {
    ...actual,
    useDecks: () => ({ decks, status: "success" as const }),
  };
});

beforeEach(async () => {
  localStorage.clear();
  installFifoWebLocks();
  await resetSavedDeckLibraryForTests();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  uninstallWebLocks();
});

describe("PreconDeckModal", () => {
  it("re-confirms overwrite when the chosen name was claimed during the transaction wait", async () => {
    // Nothing occupies "Aggro Deck (SET)" when the user is prompted (existed=false at click
    // time), so the modal's own pre-check sees no conflict — a holder claims the name only
    // once the transaction is already queued behind the lock.
    vi.stubGlobal("prompt", vi.fn(() => "Aggro Deck (SET)"));
    const confirmSpy = vi.fn(() => true);
    vi.stubGlobal("confirm", confirmSpy);
    const onImported = vi.fn();

    // The holder claims the name only once the click's own request has joined the queue, so
    // `preconExists` at click time still sees no conflict.
    const holder = withSavedDeckLibrary(async (txn) => {
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).pending).toHaveLength(1);
      });
      writeSavedDeckData(txn, "Aggro Deck (SET)", "HOLDER-DATA");
    });
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });

    render(<PreconDeckModal open onClose={vi.fn()} onImported={onImported} />);
    await userEvent.click(screen.getByRole("button", { name: /^Aggro Deck/ }));
    await holder;

    await waitFor(() => expect(confirmSpy).toHaveBeenCalledTimes(1));
    expect(onImported).toHaveBeenCalledWith("Aggro Deck (SET)");
    const persisted = JSON.parse(localStorage.getItem(STORAGE_KEY_PREFIX + "Aggro Deck (SET)") ?? "{}");
    expect(persisted.main).toEqual([{ name: "Mountain", count: 40 }]);
  });

  it("declining the in-transaction re-confirm leaves the claimed deck untouched and does not import", async () => {
    vi.stubGlobal("prompt", vi.fn(() => "Aggro Deck (SET)"));
    vi.stubGlobal("confirm", vi.fn(() => false));
    const onImported = vi.fn();

    const holder = withSavedDeckLibrary(async (txn) => {
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).pending).toHaveLength(1);
      });
      writeSavedDeckData(txn, "Aggro Deck (SET)", "HOLDER-DATA");
    });
    await vi.waitFor(async () => {
      expect((await navigator.locks.query()).held).toHaveLength(1);
    });

    render(<PreconDeckModal open onClose={vi.fn()} onImported={onImported} />);
    await userEvent.click(screen.getByRole("button", { name: /^Aggro Deck/ }));
    await holder;

    await waitFor(() => expect(vi.mocked(confirm)).toHaveBeenCalled());
    expect(onImported).not.toHaveBeenCalled();
    expect(localStorage.getItem(STORAGE_KEY_PREFIX + "Aggro Deck (SET)")).toBe("HOLDER-DATA");
  });

  it("batch import overwrites only names that conflicted at the time the user confirmed", async () => {
    localStorage.setItem(STORAGE_KEY_PREFIX + "Aggro Deck (SET)", "EXISTING-AGGRO");
    const confirmSpy = vi.fn(() => true);
    vi.stubGlobal("confirm", confirmSpy);
    const onImported = vi.fn();

    render(<PreconDeckModal open onClose={vi.fn()} onImported={onImported} />);
    await userEvent.click(screen.getByRole("checkbox", { name: /select aggro deck/i }));
    await userEvent.click(screen.getByRole("checkbox", { name: /select control deck/i }));
    await userEvent.click(screen.getByRole("button", { name: /Import \d+ selected/i }));

    await waitFor(() => expect(onImported).toHaveBeenCalled());
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    const aggro = JSON.parse(localStorage.getItem(STORAGE_KEY_PREFIX + "Aggro Deck (SET)") ?? "{}");
    expect(aggro.main).toEqual([{ name: "Mountain", count: 40 }]);
    const control = JSON.parse(localStorage.getItem(STORAGE_KEY_PREFIX + "Control Deck (SET)") ?? "{}");
    expect(control.main).toEqual([{ name: "Island", count: 40 }]);
  });
});
