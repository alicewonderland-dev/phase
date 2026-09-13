import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { clearDraftHostSession, saveDraftHostSession } = vi.hoisted(() => ({
  clearDraftHostSession: vi.fn(async () => {}),
  saveDraftHostSession: vi.fn<(id: string, session: unknown) => Promise<void>>(async () => {}),
}));

vi.mock("../../services/draftPersistence", () => ({
  clearDraftHostSession,
  saveDraftHostSession,
}));

import { P2PDraftHost } from "../p2p-draft-host";
import type { DraftPlayerView, MultiplayerSeatDescriptor } from "../draft-adapter";
import { draftProcedureFixture } from "./draftProcedureFixture";

/**
 * A Winston pod's BOT seat, from the host's side: the card database it needs to
 * exist at all, and the engine turn-loop the host must run after every human
 * decision.
 *
 * Both halves were unreachable before this change, for the same reason — the
 * reducer refused a bot seat under `PackDistribution::SharedStackPiles`, so the
 * host suppressed bot fill and nothing downstream of it ever ran. Neither half
 * is a re-spelling of the engine's own tests: `draft-wasm` owns whether the
 * loop terminates and what a bot decides, and nothing here asks either
 * question.
 */
describe("P2PDraftHost Winston bot seats", () => {
  const WINSTON_PROCEDURE = draftProcedureFixture({
    pod_size: 2,
    human_seats: 2,
    min_pod_size: 2,
    max_pod_size: 4,
    allowed_pod_sizes: [2, 3, 4],
    distribution: { SharedStackPiles: { pile_count: 3 } },
  });

  const PREMIER_PROCEDURE = draftProcedureFixture({
    pod_size: 8,
    human_seats: 8,
    allowed_pod_sizes: [2, 3, 4, 5, 6, 7, 8],
    distribution: "PickAndPass",
  });

  const originalFetch = globalThis.fetch;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    fetchMock = vi.fn(async () => new Response("CARD-DATA", { status: 200 }));
    globalThis.fetch = fetchMock as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  // ── The card database ────────────────────────────────────────────────

  function lobbyHost(kind: "Winston" | "Premier", podSize: number) {
    const host = new P2PDraftHost(
      { id: "host" } as never,
      () => () => {},
      { type: "Set", data: { pools: [{ code: "TST" }], sequence: ["TST"] } } as never,
      kind,
      podSize,
      "Host",
      "Swiss",
      "Competitive",
    );
    const createMultiplayerDraft = vi.fn(
      async (_pool: unknown, _seats: MultiplayerSeatDescriptor[]) => {},
    );
    const loadCardDatabase = vi.fn(async (_json: string) => 0);
    (host as unknown as { adapter: unknown }).adapter = {
      draftProcedure: vi.fn(async () => (kind === "Winston" ? WINSTON_PROCEDURE : PREMIER_PROCEDURE)),
      createMultiplayerDraft,
      loadCardDatabase,
      // `Lobby`, so no bot resolution and no pick timer run: these three cases
      // are about what `startDraftInner` fetches before the draft exists.
      getViewForSeat: vi.fn(async () => ({ status: "Lobby" })),
    };
    return { host, createMultiplayerDraft, loadCardDatabase };
  }

  /**
   * The database is what makes two of the five drafting principles live: the
   * valuation prices mana fixing off `produced_color_count` and cheap
   * interaction off the parsed effect profile, both of which read a `CardFace`.
   * A Set-pool Winston pod loads nothing through `draftPodHostAdapter`'s gate
   * (`poolInput.type === "Cube" || kind === "CommanderDraft"`), so without this
   * the bot would play on rarity priors alone and never say so.
   *
   * REVERT-FAILING: delete the load in `startDraftInner` and both assertions go
   * to zero calls.
   */
  it("loads the card database for a shared-stack pod that seats a bot", async () => {
    const { host, createMultiplayerDraft, loadCardDatabase } = lobbyHost("Winston", 2);
    await host.initialize();
    await host.startDraft(true);

    // Reach-guard: the pod really did seat a bot, so the fetch below is that
    // seat's doing rather than an unconditional download.
    const seats = createMultiplayerDraft.mock.calls[0]![1];
    expect(seats.filter((seat) => seat.type === "Bot")).toHaveLength(1);

    expect(fetchMock).toHaveBeenCalledOnce();
    expect(loadCardDatabase).toHaveBeenCalledWith("CARD-DATA");
    // BEFORE the session exists: `create_multiplayer_draft` deals the piles,
    // and the first bot turn can follow immediately.
    expect(loadCardDatabase.mock.invocationCallOrder[0])
      .toBeLessThan(createMultiplayerDraft.mock.invocationCallOrder[0]);
  });

  /**
   * The paired negative that keeps the fix from becoming a download regression:
   * a human-vs-human Winston pod is the common shape, and it must not pay for a
   * multi-megabyte fetch it would never read.
   *
   * REVERT-FAILING: drop the `seats.some(seat => seat.type === "Bot")` conjunct
   * and this fetches.
   */
  it("fetches nothing for a shared-stack pod with no bot seat", async () => {
    const { host, createMultiplayerDraft, loadCardDatabase } = lobbyHost("Winston", 2);
    await host.initialize();
    await host.startDraft(false);

    // Reach-guard: the start path ran to completion, with the host seat alone.
    expect(createMultiplayerDraft.mock.calls[0]![1]).toEqual([
      { type: "Human", player_id: 0, display_name: "Host" },
    ]);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(loadCardDatabase).not.toHaveBeenCalled();
  });

  /**
   * The distribution half of the same conjunct. A pick-and-pass pod full of
   * bots reads its pool from JSON and needs no database — `draftPodHostAdapter`
   * says so in place, and its landed "skips the CARD_DB fetch for Set pods" row
   * would red if this load were written as a blanket one.
   *
   * REVERT-FAILING: drop the `isSharedStackDistribution(procedure.distribution)`
   * conjunct and this fetches.
   */
  it("fetches nothing for a pick-and-pass pod full of bots", async () => {
    const { host, createMultiplayerDraft, loadCardDatabase } = lobbyHost("Premier", 8);
    await host.initialize();
    await host.startDraft(true);

    // Reach-guard: seven bot seats, and still no fetch.
    expect(createMultiplayerDraft.mock.calls[0]![1]
      .filter((seat) => seat.type === "Bot")).toHaveLength(7);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(loadCardDatabase).not.toHaveBeenCalled();
  });

  // ── The turn loop after a human decision ─────────────────────────────

  /**
   * `active_seat` says whose turn it is. Seat 1 is the BOT when the pod seated
   * one; the human-only variant is the same pod with that flag false, so the
   * two cases differ in exactly the field the host dispatches on.
   */
  function winstonView(activeSeat: number, status: string, seatABot: boolean): DraftPlayerView {
    return {
      status,
      kind: "Winston",
      pool: [],
      current_pack: null,
      required_pick_count: 0,
      draft_effects: [],
      seats: [
        { seat_index: 0, display_name: "Host", is_bot: false, connected: true,
          has_submitted_deck: false, pick_status: "Pending", active_pack_count: 0,
          face_up_draft_cards: [] },
        { seat_index: 1, display_name: "Guest", is_bot: seatABot, connected: true,
          has_submitted_deck: false, pick_status: "Pending", active_pack_count: 0,
          face_up_draft_cards: [] },
      ],
      pick_number: 0,
      shared_stack: {
        main_stack_remaining: 11,
        total_cards: 20,
        active_seat: activeSeat,
        active_pile: 0,
        piles: [],
        decisions: 5,
        history: [],
      },
    } as unknown as DraftPlayerView;
  }

  /**
   * Starts a Winston pod with one bot seat and hands back the mocks the two
   * cases below assert on. `resolveSharedStackBotTurns` flips the view the host
   * reads, exactly as the engine would: the turn comes back to the human.
   */
  async function startedPodWithABot(
    finalStatus: "Drafting" | "Deckbuilding",
    { seatABot = true }: { seatABot?: boolean } = {},
  ) {
    vi.useFakeTimers();
    const host = new P2PDraftHost(
      { id: "host" } as never,
      () => () => {},
      { type: "Set", data: { pools: [{ code: "TST" }], sequence: ["TST"] } } as never,
      "Winston",
      2,
      "Host",
      "Swiss",
      "Competitive",
      undefined,
      "winston-bot-pod",
      "ABCDE",
    );
    let botsHaveRun = false;
    const resolveSharedStackBotTurns = vi.fn(async () => {
      botsHaveRun = true;
      return [{ SharedStackDecisionApplied: { seat: 1 } }];
    });
    const getViewForSeat = vi.fn(async () =>
      (botsHaveRun ? winstonView(0, finalStatus, seatABot) : winstonView(1, "Drafting", seatABot)));
    const submitSharedStackDecisionForSeat = vi.fn(
      async () => winstonView(1, "Drafting", seatABot));
    (host as unknown as { adapter: unknown }).adapter = {
      draftProcedure: vi.fn(async () => WINSTON_PROCEDURE),
      createMultiplayerDraft: vi.fn(async () => {}),
      loadCardDatabase: vi.fn(async () => 0),
      exportSession: vi.fn(async () => "{}"),
      allPicksSubmitted: vi.fn(async () => false),
      getViewForSeat,
      submitSharedStackDecisionForSeat,
      resolveSharedStackBotTurns,
    };
    await host.initialize();
    await host.startDraft(seatABot);
    // The start path resolves the first bot turns too; reset so the assertions
    // below read the DECISION path alone.
    botsHaveRun = false;
    resolveSharedStackBotTurns.mockClear();
    saveDraftHostSession.mockClear();
    return {
      host,
      resolveSharedStackBotTurns,
      submitSharedStackDecisionForSeat,
      privateHost: host as unknown as {
        timerContext: string | null;
        timerInterval: ReturnType<typeof setInterval> | null;
      },
    };
  }

  /**
   * The seam this phase opens: a human decision passes the turn to a bot, and
   * the host must hand that turn to the engine's own loop and make the result
   * durable before anyone sees it.
   *
   * REVERT-FAILING: delete the `resolveBotPicks` call from
   * `handleSharedStackDecision` and the bot never moves (0 calls), leaving the
   * pod stalled on a seat with no player in it.
   */
  it("runs the engine's bot turn loop after an applied decision, and fences it", async () => {
    const { host, resolveSharedStackBotTurns, submitSharedStackDecisionForSeat, privateHost } =
      await startedPodWithABot("Drafting");

    await host.submitHostSharedStackDecision(0, "Take");

    // Reach-guard: the human's decision really reached the reducer, so the bot
    // work below follows an applied decision rather than an empty call.
    expect(submitSharedStackDecisionForSeat).toHaveBeenCalledWith(0, 0, "Take");
    expect(resolveSharedStackBotTurns).toHaveBeenCalledOnce();
    // Two fences: the human's decision, then the bot turns' own.
    expect(saveDraftHostSession).toHaveBeenCalledTimes(2);
    // The clock is re-armed for the seat the bot chain stopped on.
    expect(privateHost.timerContext).toBe("pick");
    expect(privateHost.timerInterval).not.toBeNull();
  });

  /**
   * The paired negative for the loop, and the guard on the `is_bot` pre-check:
   * a human-vs-human Winston pod — the common shape — makes NO engine
   * round-trip on any decision. The export would answer honestly (an empty
   * list when the active seat is human), so this is an economy rather than a
   * legality test, and it is the reason `p2pDraftHostWinstonTimer.test.ts`
   * still passes with its adapter mock untouched.
   *
   * REVERT-FAILING: delete the `hostView.seats.some(seat => seat.is_bot)`
   * pre-check in `resolveSharedStackBotTurns` and this reds with one call.
   */
  it("makes no bot round-trip for a shared-stack pod of humans", async () => {
    const { host, resolveSharedStackBotTurns, submitSharedStackDecisionForSeat } =
      await startedPodWithABot("Drafting", { seatABot: false });

    await host.submitHostSharedStackDecision(0, "Take");

    // Reach-guard: the decision path really ran, so "not called" is a decision
    // rather than a path this fixture never entered.
    expect(submitSharedStackDecisionForSeat).toHaveBeenCalledWith(0, 0, "Take");
    expect(resolveSharedStackBotTurns).not.toHaveBeenCalled();
  });

  /**
   * The ORDERING discriminator, and the reason the host view is re-read after
   * the bot turns rather than before: a bot chain can end the draft. Read too
   * early and the host arms a pick clock on a finished draft and never reports
   * it complete.
   *
   * REVERT-FAILING: move the `getViewForSeat(0)` re-read above the
   * `resolveBotPicks` call and this reds — the early view still says
   * `Drafting`.
   */
  it("reports a draft the bot turns finished, instead of arming a dead clock", async () => {
    const { host, privateHost } = await startedPodWithABot("Deckbuilding");
    const events: string[] = [];
    host.onEvent((event) => events.push(event.type));

    await host.submitHostSharedStackDecision(0, "Decline");

    expect(events).toContain("draftComplete");
    expect(privateHost.timerInterval).toBeNull();
  });
});
