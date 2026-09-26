import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
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
    await waitFor(() => expect(onImported).toHaveBeenCalledWith("Aggro Deck (SET)", "open"));
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

  describe("closing and reopening while a save waits", () => {
    function Harness({
      onImported,
      onClose,
    }: {
      onImported: (name: string, session: "open" | "dismissed") => void;
      onClose: () => void;
    }) {
      const [open, setOpen] = useState(true);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>reopen</button>
          <PreconDeckModal open={open} onClose={() => { onClose(); setOpen(false); }} onImported={onImported} />
        </>
      );
    }

    it("a pick whose save finishes after the modal was closed and reopened leaves the reopened modal open", async () => {
      const onImported = vi.fn();
      const onClose = vi.fn();
      let releaseHolder!: () => void;
      const held = new Promise<void>((resolve) => { releaseHolder = resolve; });
      const holder = withSavedDeckLibrary(() => held);
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).held).toHaveLength(1);
      });

      vi.stubGlobal("prompt", vi.fn(() => "Aggro Deck (SET)"));
      render(<Harness onImported={onImported} onClose={onClose} />);
      await userEvent.click(screen.getByRole("button", { name: /^Aggro Deck/ }));
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).pending).toHaveLength(1);
      });

      await userEvent.keyboard("{Escape}");
      await userEvent.click(screen.getByRole("button", { name: "reopen" }));
      releaseHolder();
      await holder;

      await waitFor(() => expect(onImported).toHaveBeenCalledWith("Aggro Deck (SET)", "dismissed"));
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("button", { name: /^Aggro Deck/ })).toBeInTheDocument();
    });

    it("a batch import that finishes after the modal was closed and reopened leaves the reopened modal and its selection alone", async () => {
      const onImported = vi.fn();
      const onClose = vi.fn();
      let releaseHolder!: () => void;
      const held = new Promise<void>((resolve) => { releaseHolder = resolve; });
      const holder = withSavedDeckLibrary(() => held);
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).held).toHaveLength(1);
      });

      render(<Harness onImported={onImported} onClose={onClose} />);
      // Only Aggro is part of this batch — Control is left free so it can be
      // picked after reopening without overlapping the batch's own ids.
      await userEvent.click(screen.getByRole("checkbox", { name: /select aggro deck/i }));
      await userEvent.click(screen.getByRole("button", { name: /Import \d+ selected/i }));
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).pending).toHaveLength(1);
      });

      await userEvent.keyboard("{Escape}");
      await userEvent.click(screen.getByRole("button", { name: "reopen" }));
      // A pick made after reopening, while the dismissed batch is still queued.
      await userEvent.click(screen.getByRole("checkbox", { name: /select control deck/i }));
      releaseHolder();
      await holder;

      await waitFor(() => expect(onImported).toHaveBeenCalledTimes(1));
      expect(onImported).toHaveBeenCalledWith("Aggro Deck (SET)", "dismissed");
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("checkbox", { name: /select aggro deck/i })).not.toBeChecked();
      expect(screen.getByRole("checkbox", { name: /select control deck/i })).toBeChecked();
    });

    it("a second writer claiming the chosen name is not re-confirmed once the modal was dismissed", async () => {
      vi.stubGlobal("prompt", vi.fn(() => "Aggro Deck (SET)"));
      const confirmSpy = vi.fn(() => true);
      vi.stubGlobal("confirm", confirmSpy);
      const onImported = vi.fn();
      const onClose = vi.fn();

      let resolveDismissed!: () => void;
      const dismissedP = new Promise<void>((resolve) => { resolveDismissed = resolve; });
      const holder = withSavedDeckLibrary(async (txn) => {
        await vi.waitFor(async () => {
          expect((await navigator.locks.query()).pending).toHaveLength(1);
        });
        // Don't write until the test has dismissed the modal — otherwise this
        // holder can settle the write and release the lock before the
        // {Escape} keypress below has been dispatched, racing the assertion
        // that a dismissed session skips the re-confirm.
        await dismissedP;
        writeSavedDeckData(txn, "Aggro Deck (SET)", "HOLDER-DATA");
      });
      await vi.waitFor(async () => {
        expect((await navigator.locks.query()).held).toHaveLength(1);
      });

      render(<Harness onImported={onImported} onClose={onClose} />);
      await userEvent.click(screen.getByRole("button", { name: /^Aggro Deck/ }));
      await userEvent.keyboard("{Escape}");
      resolveDismissed();
      await holder;

      // The save observed "kept-existing" (another writer claimed the name), but the
      // modal session that started this pick is no longer open, so no second confirm.
      // Reach guard: wait for the click's own (now-unblocked) save to finish running.
      await vi.waitFor(async () => {
        const q = await navigator.locks.query();
        expect(q.held).toHaveLength(0);
        expect(q.pending).toHaveLength(0);
      });
      expect(confirmSpy).not.toHaveBeenCalled();
      expect(onImported).not.toHaveBeenCalled();
    });

    it("a second writer claiming the chosen name is re-confirmed while the modal stays open" , async () => {
      vi.stubGlobal("prompt", vi.fn(() => "Aggro Deck (SET)"));
      const confirmSpy = vi.fn(() => true);
      vi.stubGlobal("confirm", confirmSpy);
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

      await waitFor(() => expect(confirmSpy).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(onImported).toHaveBeenCalledWith("Aggro Deck (SET)", "open"));
    });
  });
});
