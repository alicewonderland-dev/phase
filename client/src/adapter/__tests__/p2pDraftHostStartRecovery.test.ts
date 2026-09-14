import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { clearDraftHostSession, loadDraftHostSession, saveDraftHostSession } = vi.hoisted(() => ({
  clearDraftHostSession: vi.fn(async () => {}),
  loadDraftHostSession: vi.fn(async () => null),
  saveDraftHostSession: vi.fn<(id: string, session: unknown) => Promise<void>>(async () => {}),
}));
vi.mock("../../services/draftPersistence", () => ({
  clearDraftHostSession,
  loadDraftHostSession,
  saveDraftHostSession,
}));

import { P2PDraftHost } from "../p2p-draft-host";
import type { DraftProcedure } from "../draft-adapter";
import { draftProcedureFixture } from "./draftProcedureFixture";

/**
 * A FAILED START MUST STAY RETRYABLE.
 *
 * `startDraftInner` opens with `if (this.draftStarted) return`. It used to raise
 * that flag before the bot loop and before the durable snapshot, so a throw from
 * either left it standing over a pod that does not exist anywhere a client can
 * see: the engine holds a draft, no snapshot was written, no guest was told, and
 * every retry hits the guard and returns silently. The host looks idle and the
 * Start button does nothing, forever.
 *
 * The flags come back down on failure now. What this pins is the RETRY, not the
 * rejection -- a test that only asserted `rejects` would pass with the bug fully
 * intact, because the first call threw either way.
 */
describe("P2PDraftHost start recovery", () => {
  const originalFetch = globalThis.fetch;

  beforeEach(() => {
    globalThis.fetch = vi.fn(async () => new Response("{}", { status: 200 })) as typeof fetch;
    saveDraftHostSession.mockReset();
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  function hostWithAdapter() {
    const procedure: DraftProcedure = draftProcedureFixture({
      pod_size: 2,
      human_seats: 2,
      distribution: { SharedStackPiles: { pile_count: 3 } },
      allowed_pod_sizes: [2, 3, 4],
    });
    const host = new P2PDraftHost(
      { id: "host" } as never,
      () => () => {},
      { type: "Set", data: { pools: [{ code: "TST" }], sequence: ["TST"] } } as never,
      "Winston",
      2,
      "Host",
      "Swiss",
      "Competitive",
    );
    const createMultiplayerDraft = vi.fn(async () => {});
    (host as unknown as { adapter: unknown }).adapter = {
      draftProcedure: vi.fn(async () => procedure),
      createMultiplayerDraft,
      // `Lobby`, so the bot loop is skipped and the persist below is the only
      // thing that can fail -- the failure this test is about.
      getViewForSeat: vi.fn(async () => ({ status: "Lobby" })),
      exportSession: vi.fn(async () => ({})),
      loadCardDatabase: vi.fn(async () => 0),
    };
    // Persistence is a no-op without an id, which would make the fixture unable
    // to fail at all.
    (host as unknown as { persistenceId: string }).persistenceId = "start-recovery";
    return { host, createMultiplayerDraft };
  }

  it("retries a start whose durable snapshot failed", async () => {
    const { host, createMultiplayerDraft } = hostWithAdapter();
    // AFTER `initialize`, which persists once on its own -- arming the rejection
    // before it would spend the single failure on the wrong write and leave the
    // start to succeed, which is exactly how this test first failed.
    saveDraftHostSession.mockResolvedValue(undefined);
    await host.initialize();
    saveDraftHostSession
      .mockRejectedValueOnce(new Error("IndexedDB unavailable"))
      .mockResolvedValue(undefined);

    await expect(host.startDraft(true)).rejects.toThrow("IndexedDB unavailable");

    // Reach guard: the first attempt really did get as far as the engine.
    expect(createMultiplayerDraft).toHaveBeenCalledOnce();

    // THE CLAIM. Leave `draftStarted` standing on the failure path and this
    // second call returns at the guard, `createMultiplayerDraft` stays at one,
    // and the pod is stranded with the host reporting nothing wrong.
    await expect(host.startDraft(true)).resolves.toBeUndefined();
    expect(createMultiplayerDraft).toHaveBeenCalledTimes(2);
  });

  it("does not restart a draft that started cleanly", async () => {
    // The paired positive: the rollback must not weaken the guard it rolls back.
    // Without this, "always allow a restart" would pass the test above.
    const { host, createMultiplayerDraft } = hostWithAdapter();
    saveDraftHostSession.mockResolvedValue(undefined);

    await host.initialize();
    await host.startDraft(true);
    expect(createMultiplayerDraft).toHaveBeenCalledOnce();

    await host.startDraft(true);
    expect(createMultiplayerDraft).toHaveBeenCalledOnce();
  });
});
