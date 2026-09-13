import { describe, expect, it, vi } from "vitest";

import { P2PDraftHost } from "../p2p-draft-host";
import type { DraftKind, DraftProcedure, MultiplayerSeatDescriptor, PackDistribution } from "../draft-adapter";
import { draftProcedureFixture } from "./draftProcedureFixture";

/**
 * Host-side bot fill must be suppressed for EXACTLY the kinds the engine
 * refuses a bot seat for, and for no others.
 *
 * The engine's authority is a match on `PackDistribution::SharedStackPiles`:
 * `apply_start_draft`'s pre-flight arm and `apply_replace_seat_with_bot` both
 * dispatch on the distribution, and `apply_start_draft` states in place that
 * `human_seats` is "necessary but not sufficient". The host previously asked
 * `procedure.human_seats !== procedure.pod_size` instead, which is a scalar
 * that merely CORRELATES — and correlates wrongly, because the engine's
 * procedure table seats humans in every seat for Premier, Traditional and
 * Sealed as well (`human_seats == pod_size == 8`). That silently disabled bot
 * fill for three kinds that permit it, leaving a one-human pod to fail
 * `createMultiplayerDraft`'s `min_pod_size` floor after `canStart` had already
 * passed on `botFillEnabled`.
 *
 * The rows below therefore carry the REAL engine scalars, because a fixture
 * with `human_seats: 1` (the shared `draftProcedureFixture` default) is
 * exactly what let the regression through: under it both predicates agree.
 */
describe("P2PDraftHost bot fill dispatches on the engine's refusal property", () => {
  /**
   * `human_seats` and `pod_size` mirror `DraftKind::procedure()` in
   * `crates/draft-core/src/types.rs`. `botFillPermitted` is the ENGINE's
   * answer, derived from the distribution, not from the scalars.
   */
  // @sync-with: crates/draft-core/src/types.rs `DraftKind::procedure`
  const ROWS: ReadonlyArray<{
    kind: Exclude<DraftKind, "Quick">;
    pod_size: number;
    human_seats: number;
    distribution: PackDistribution;
    botFillPermitted: boolean;
  }> = [
    // The three rows the scalar predicate got WRONG: humans in every seat, yet
    // `PickAndPass`, so the reducer accepts a bot seat and bot fill is legal.
    { kind: "Premier", pod_size: 8, human_seats: 8, distribution: "PickAndPass", botFillPermitted: true },
    { kind: "Traditional", pod_size: 8, human_seats: 8, distribution: "PickAndPass", botFillPermitted: true },
    { kind: "Sealed", pod_size: 8, human_seats: 8, distribution: "AllAtOnce", botFillPermitted: true },
    // The row both predicates happened to agree on.
    { kind: "CommanderDraft", pod_size: 4, human_seats: 1, distribution: "PickAndPass", botFillPermitted: true },
    // The only row the engine actually refuses, and the only one whose
    // `human_seats === pod_size` is MEANT to coincide with the refusal.
    {
      kind: "Winston",
      pod_size: 2,
      human_seats: 2,
      distribution: { SharedStackPiles: { pile_count: 3 } },
      botFillPermitted: false,
    },
  ];

  async function startWith(row: (typeof ROWS)[number], botFillEmptySeats: boolean) {
    const procedure: DraftProcedure = draftProcedureFixture({
      pod_size: row.pod_size,
      human_seats: row.human_seats,
      distribution: row.distribution,
      allowed_pod_sizes: [2, 3, 4, 5, 6, 7, 8],
    });
    const host = new P2PDraftHost(
      { id: "host" } as never,
      () => () => {},
      { type: "Set", data: { pools: [{ code: "TST" }], sequence: ["TST"] } } as never,
      row.kind,
      row.pod_size,
      "Host",
      "Swiss",
      "Competitive",
    );
    // Typed on the seat parameter specifically: the descriptor list is the whole
    // subject of this suite, so reading it back as `unknown` would defeat the
    // assertions below.
    const createMultiplayerDraft = vi.fn(
      async (_pool: unknown, _seats: MultiplayerSeatDescriptor[]) => {},
    );
    // `Lobby`, so neither `resolveBotPicks` nor `startPickTimer` runs: this
    // test is about the seat descriptors `startDraft` hands the engine.
    const getViewForSeat = vi.fn(async () => ({ status: "Lobby" }));
    (host as unknown as { adapter: unknown }).adapter = {
      draftProcedure: vi.fn(async () => procedure),
      createMultiplayerDraft,
      getViewForSeat,
    };

    await host.initialize();
    await host.startDraft(botFillEmptySeats);

    expect(createMultiplayerDraft).toHaveBeenCalledOnce();
    return createMultiplayerDraft.mock.calls[0]![1];
  }

  /**
   * REVERT-PROBE — this is the test the regression needed and did not have.
   * Restore `botFillEmptySeats && procedure.human_seats !== procedure.pod_size`
   * and the three `human_seats === pod_size` rows red here with zero bot seats.
   */
  it.each(ROWS.filter((row) => row.botFillPermitted))(
    "fills empty $kind seats with bots even when human_seats equals pod_size",
    async (row) => {
      const seats = await startWith(row, true);

      // Reach-guard: the host seat is present, so `startDraft` really built
      // the descriptor list rather than short-circuiting somewhere earlier.
      // Without this, the bot count below could not tell "bot fill ran" from
      // "nothing ran at all".
      expect(seats.filter((seat) => seat.type === "Human")).toEqual([
        { type: "Human", player_id: 0, display_name: "Host" },
      ]);
      // Every seat the pod declares is filled, which is precisely what keeps
      // `createMultiplayerDraft` clear of the `min_pod_size` floor.
      expect(seats).toHaveLength(row.pod_size);
      expect(seats.filter((seat) => seat.type === "Bot")).toHaveLength(row.pod_size - 1);
    },
  );

  /**
   * The paired negative, and it is NOT vacuous: the reach-guard above proves
   * the descriptor list gets built for every kind, so an empty bot count here
   * is a suppression rather than an absent code path. Flipping the guard to a
   * bare `botFillEmptySeats` reds this.
   */
  it.each(ROWS.filter((row) => !row.botFillPermitted))(
    "never seats a bot in a $kind pod, whose distribution the reducer refuses",
    async (row) => {
      const seats = await startWith(row, true);

      expect(seats).toEqual([{ type: "Human", player_id: 0, display_name: "Host" }]);
      expect(seats.filter((seat) => seat.type === "Bot")).toHaveLength(0);
    },
  );

  /**
   * The host's own opt-out still wins for a kind that permits bot fill, so the
   * distribution test did not become the ONLY input to the guard.
   */
  it("leaves a permitting pod short when the host declines bot fill", async () => {
    const seats = await startWith(ROWS[0]!, false);

    expect(seats).toEqual([{ type: "Human", player_id: 0, display_name: "Host" }]);
  });
});
