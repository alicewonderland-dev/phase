import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PackDistribution } from "../../../adapter/draft-adapter";
import { useConnectivityStore } from "../../../stores/connectivityStore";
import { DRAFT_OFFLINE_ERROR } from "../../../stores/multiplayerDraftStore";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  startDraft: vi.fn(async () => {}),
  toggleBotFill: vi.fn(),
  kickPlayer: vi.fn(),
  leave: vi.fn(async () => {}),
  copyText: vi.fn(),
  multiplayerState: {
    role: "host",
    seats: [
      {
        seat_index: 0,
        display_name: "Host",
        is_bot: false,
        connected: true,
        has_submitted_deck: false,
        pick_status: "NotDrafting",
      },
    ],
    joined: 1,
    total: 4,
    roomCode: "ABCDE",
    seatIndex: 0,
    error: null as string | null,
  },
  podState: {
    botFillEnabled: true,
    // The engine-published seat counts the lobby's Start gate reads. The base
    // Premier procedure allows every normal pod size from two through eight.
    allowedPodSizes: [2, 3, 4, 5, 6, 7, 8] as number[] | null,
    // The engine-published distribution. The Start gate reads it to decide
    // whether bot fill can pad the pod at all; `null` until the procedure loads.
    packDistribution: "PickAndPass" as PackDistribution | null,
    config: {
      setCode: "dft",
      setName: "Draft Set",
      kind: "Premier",
      podSize: 4,
    },
  },
}));

type MultiplayerMockState = typeof mocks.multiplayerState & {
  kickPlayer: typeof mocks.kickPlayer;
  leave: typeof mocks.leave;
  startDraft: typeof mocks.startDraft;
};

type PodMockState = typeof mocks.podState & {
  toggleBotFill: typeof mocks.toggleBotFill;
  startDraft: () => Promise<void>;
};

vi.mock("../../../stores/multiplayerDraftStore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../stores/multiplayerDraftStore")>();
  const state = (): MultiplayerMockState => ({
      ...mocks.multiplayerState,
      kickPlayer: mocks.kickPlayer,
      leave: mocks.leave,
      startDraft: mocks.startDraft,
    });
  return {
    ...actual,
    useMultiplayerDraftStore: Object.assign(
      (selector: (current: MultiplayerMockState) => unknown) => selector(state()),
      { getState: state },
    ),
  };
});

// Only the hook is stubbed. `draftKindLabels` — the single authority for rendering
// a `DraftKind` as prose — lives in the leaf module `components/draft/draftKind`
// and is not mocked: a stub would be a second copy of the map and could not catch
// it drifting.
vi.mock("../../../stores/draftPodStore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../stores/draftPodStore")>();
  return {
    ...actual,
    useDraftPodStore: (selector: (state: PodMockState) => unknown) =>
      selector({
      ...mocks.podState,
      toggleBotFill: mocks.toggleBotFill,
      startDraft: actual.useDraftPodStore.getState().startDraft,
      }),
  };
});
vi.mock("../../../services/copyText", () => ({ copyText: mocks.copyText }));

import { DraftPodLobby } from "../DraftPodLobby";

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("DraftPodLobby", () => {
  beforeEach(() => {
    mocks.startDraft.mockReset();
    mocks.startDraft.mockResolvedValue(undefined);
    mocks.toggleBotFill.mockClear();
    mocks.kickPlayer.mockClear();
    mocks.leave.mockReset();
    mocks.leave.mockResolvedValue(undefined);
    mocks.copyText.mockClear();
    useConnectivityStore.setState({ forcedOffline: false, browserOnline: true });
  });

  afterEach(() => {
    cleanup();
    useConnectivityStore.setState({ forcedOffline: false, browserOnline: true });
  });

  it("shows the host in the first seat and allows starting with bot fill", () => {
    render(<DraftPodLobby onLeave={vi.fn()} />);

    expect(screen.getByText("Host")).toBeInTheDocument();
    expect(screen.getByText("HOST")).toBeInTheDocument();
    expect(screen.getByText("1 / 4 seats filled")).toBeInTheDocument();

    const startButton = screen.getByRole("button", { name: "Start Draft" });
    expect(startButton).toBeEnabled();

    fireEvent.click(startButton);

    expect(mocks.startDraft).toHaveBeenCalledTimes(1);
  });

  it("does not reach the lower start seam when connectivity flips before an online-rendered Start click", async () => {
    render(<DraftPodLobby onLeave={vi.fn()} />);
    const startButton = screen.getByRole("button", { name: "Start Draft" });
    expect(startButton).toBeEnabled();

    // The browser can deliver the click already queued from the online render
    // after connectivity changes but before React paints the disabled control.
    // This calls the REAL draft-pod public action, whose only lower seam is the
    // mocked multiplayer start below.
    useConnectivityStore.setState({ forcedOffline: true });
    expect(startButton).toBeEnabled();
    fireEvent.click(startButton);

    await vi.waitFor(() => expect(mocks.startDraft).not.toHaveBeenCalled());
  });

  it("names the draft kind in prose rather than as a raw enum", () => {
    mocks.podState.config.kind = "CommanderDraft";
    render(<DraftPodLobby onLeave={vi.fn()} />);

    // Reach guard: the header rendered, so the string below is a real reading.
    expect(screen.getByText("Draft Pod Lobby")).toBeInTheDocument();
    // REVERT-FAILING: BASE interpolates `config.kind` directly, producing
    // "CommanderDraft Draft" once Commander Draft is selectable.
    expect(screen.getByText(/Commander Draft/)).toBeInTheDocument();
    expect(screen.queryByText(/CommanderDraft/)).toBeNull();
  });

  it("still names the pre-existing kinds from the same map", () => {
    mocks.podState.config.kind = "Premier";
    render(<DraftPodLobby onLeave={vi.fn()} />);

    expect(screen.getByText(/Premier Draft/)).toBeInTheDocument();
  });

  it.each([
    ["forced offline", { forcedOffline: true, browserOnline: true }],
    ["browser offline", { forcedOffline: false, browserOnline: false }],
  ])("disables only Start and preserves host lobby controls while %s", async (_label, connectivity) => {
    const originalSeats = mocks.multiplayerState.seats;
    const originalError = mocks.multiplayerState.error;
    mocks.multiplayerState.seats = [
      ...originalSeats,
      {
        seat_index: 1,
        display_name: "Guest",
        is_bot: false,
        connected: true,
        has_submitted_deck: false,
        pick_status: "NotDrafting",
      },
    ];
    mocks.multiplayerState.error = DRAFT_OFFLINE_ERROR;
    useConnectivityStore.setState(connectivity);
    const onLeave = vi.fn();
    const leaveCompletion = deferred<void>();
    mocks.leave.mockImplementationOnce(() => leaveCompletion.promise);
    render(<DraftPodLobby onLeave={onLeave} />);

    expect(screen.getByText("Reconnect or turn off Offline Mode to host, join, start, or watch a multiplayer draft.")).toBeInTheDocument();
    expect(screen.getByText("Starting a multiplayer draft is unavailable while offline. Reconnect or turn off Offline Mode to continue.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start Draft" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Kick" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Fill empty seats with bots" }));
    fireEvent.click(screen.getByRole("button", { name: /ABCDE/ }));
    fireEvent.click(screen.getByRole("button", { name: "Leave" }));

    expect(mocks.kickPlayer).toHaveBeenCalledWith(1);
    expect(mocks.toggleBotFill).toHaveBeenCalledTimes(1);
    expect(mocks.copyText).toHaveBeenCalledWith("ABCDE");
    expect(mocks.leave).toHaveBeenCalledTimes(1);
    expect(onLeave).not.toHaveBeenCalled();

    await act(async () => {
      leaveCompletion.resolve();
      await leaveCompletion.promise;
    });

    expect(onLeave).toHaveBeenCalledTimes(1);
    mocks.multiplayerState.seats = originalSeats;
    mocks.multiplayerState.error = originalError;
  });
  /**
   * `canStart` reads the complete engine-published allowed-size set from the
   * store. No kind-blind floor or fallback is reconstructed in the UI.
   */
  describe("the Start gate reads the engine-published allowed seat counts", () => {
    const baseSeats = mocks.multiplayerState.seats;
    const baseJoined = mocks.multiplayerState.joined;
    const baseBotFill = mocks.podState.botFillEnabled;
    const baseAllowedPodSizes = mocks.podState.allowedPodSizes;
    const baseDistribution = mocks.podState.packDistribution;

    /** `filled` occupied seats out of four. `DraftPodLobby` counts a seat as
     *  filled by its `display_name`, so the empties carry none. */
    function seatsFilled(filled: number) {
      return Array.from({ length: 4 }, (_, i) => ({
        seat_index: i,
        display_name: i < filled ? `P${i}` : "",
        is_bot: false,
        connected: true,
        has_submitted_deck: false,
        pick_status: "NotDrafting",
      }));
    }

    function startButton() {
      return screen.getByRole("button", { name: "Start Draft" });
    }

    beforeEach(() => {
      mocks.multiplayerState.seats = seatsFilled(2);
      mocks.multiplayerState.joined = 2;
      mocks.podState.botFillEnabled = false;
    });

    afterEach(() => {
      mocks.multiplayerState.seats = baseSeats;
      mocks.multiplayerState.joined = baseJoined;
      mocks.podState.botFillEnabled = baseBotFill;
      mocks.podState.allowedPodSizes = baseAllowedPodSizes;
      mocks.podState.packDistribution = baseDistribution;
    });

    it("disables Start when two seats are outside Commander Draft's allowed set", () => {
      mocks.podState.allowedPodSizes = [3, 4, 5, 6, 7, 8];
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // Reach guard: the lobby really rendered these two seats, so the
      // disabled state below is a reading of THIS fixture.
      expect(screen.getByText("2 / 4 seats filled")).toBeInTheDocument();
      // REVERT-FAILING: a kind-blind two-seat fallback makes this enabled.
      expect(startButton()).toBeDisabled();
    });

    it("enables Start when two seats are in Premier's allowed set", () => {
      mocks.podState.allowedPodSizes = [2, 3, 4, 5, 6, 7, 8];
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // The paired positive reach-guard: without it the negative above is
      // satisfiable by a button that is never enabled at all.
      expect(startButton()).toBeEnabled();
    });

    it("lets bot-fill enable Start outside the current allowed set", () => {
      mocks.podState.allowedPodSizes = [3, 4, 5, 6, 7, 8];
      mocks.podState.botFillEnabled = true;
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // Multi-authority: bot-fill pads the pod to `procedure.pod_size`, which
      // is above every kind's floor, so its short-circuit is preserved.
      expect(startButton()).toBeEnabled();
    });

    /**
     * The shared-stack arm of the same gate, and the reason it exists: the
     * reducer refuses a bot seat under `PackDistribution::SharedStackPiles`, so
     * the host's bot-fill checkbox pads NOTHING there. Letting it short-circuit
     * the seat-count test hands the host an enabled Start that fails
     * `createMultiplayerDraft`'s `min_pod_size` floor.
     *
     * REVERT-FAILING: drop the `isSharedStackDistribution` conjunct from
     * `botFillPadsThePod` and this enables, exactly as it did before the fix.
     */
    it("does not let bot-fill enable Start for a shared-stack pod, which seats no bots", () => {
      mocks.podState.allowedPodSizes = [3, 4, 5, 6, 7, 8];
      mocks.podState.botFillEnabled = true;
      mocks.podState.packDistribution = { SharedStackPiles: { pile_count: 3 } };
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // Reach guard: the same two-seat fixture the PickAndPass case above
      // renders, so the difference is the distribution and nothing else.
      expect(screen.getByText("2 / 4 seats filled")).toBeInTheDocument();
      expect(startButton()).toBeDisabled();
    });

    /**
     * The paired positive: a shared-stack pod whose HUMANS already make a legal
     * seat count still starts. Without this, the assertion above is satisfiable
     * by a gate that refuses every Winston pod forever.
     */
    it("still starts a shared-stack pod once the humans reach an allowed seat count", () => {
      mocks.podState.allowedPodSizes = [2, 3, 4];
      mocks.podState.botFillEnabled = true;
      mocks.podState.packDistribution = { SharedStackPiles: { pile_count: 3 } };
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(startButton()).toBeEnabled();
    });

    it("disables Start while the engine has not answered", () => {
      mocks.podState.allowedPodSizes = null;
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // Fail closed: no client-side fallback may reinstate a legal count.
      expect(startButton()).toBeDisabled();
    });

    /**
     * The seat GRID reads the same authority as the Start gate. An empty seat
     * in a shared-stack pod must not be labelled "Bot": no bot will ever fill
     * it, and the label is what tells the host the pod is already accounted
     * for when in fact it is short.
     *
     * REVERT-FAILING: pass `botFillEnabled` to `SeatCard` again and the empty
     * seats read "Bot" here.
     */
    it("does not label empty shared-stack seats as bots", () => {
      mocks.podState.botFillEnabled = true;
      mocks.podState.packDistribution = { SharedStackPiles: { pile_count: 3 } };
      render(<DraftPodLobby onLeave={vi.fn()} />);

      // Reach guard: two of the four seats really are empty in this fixture.
      expect(screen.getByText("2 / 4 seats filled")).toBeInTheDocument();
      expect(screen.queryAllByText("Bot")).toHaveLength(0);
      expect(screen.queryAllByText("Waiting...")).toHaveLength(2);
    });

    /**
     * The paired positive, same fixture and same checkbox: a PickAndPass pod
     * does promise bots for its empty seats, because it really gets them.
     */
    it("still labels empty seats as bots where bot fill seats them", () => {
      mocks.podState.botFillEnabled = true;
      mocks.podState.packDistribution = "PickAndPass";
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(screen.queryAllByText("Bot")).toHaveLength(2);
    });

    /**
     * The distribution half of the same fail-closed rule. `botFillEnabled` may
     * not short-circuit the seat-count test before the engine has said whether
     * bot seats are legal for this kind at all.
     */
    it("disables Start while the engine has not published the distribution", () => {
      mocks.podState.allowedPodSizes = [3, 4, 5, 6, 7, 8];
      mocks.podState.botFillEnabled = true;
      mocks.podState.packDistribution = null;
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(startButton()).toBeDisabled();
    });

    /**
     * The last form of the same defect the Start gate and the seat labels
     * already close: a control that offers a capability the engine refuses.
     * `botFillPadsThePod` makes the toggle inert for a shared-stack pod, so
     * leaving it on screen shows the host a switch that provably does nothing.
     *
     * REVERT-FAILING: render the label unconditionally again and the checkbox
     * reappears in the shared-stack case below.
     */
    it("hides the bot-fill toggle for a pod whose procedure refuses bot seats", () => {
      mocks.podState.packDistribution = { SharedStackPiles: { pile_count: 3 } };
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(screen.queryByText("Fill empty seats with bots")).not.toBeInTheDocument();
    });

    /**
     * The paired positive, and the reach-guard for the assertion above: the
     * same render path DOES show the toggle wherever bot seats are legal, so
     * "not in the document" is a decision about this procedure rather than a
     * control this harness never renders at all.
     */
    it("keeps the bot-fill toggle for a pod whose procedure seats bots", () => {
      mocks.podState.packDistribution = "PickAndPass";
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(screen.queryByText("Fill empty seats with bots")).toBeInTheDocument();
    });

    /**
     * The `null` sides of the two predicates are deliberately opposite, and
     * this pins the half that is easy to "tidy" into agreement: with no
     * procedure loaded the engine has not refused anything, so the control
     * stays rather than vanishing and reappearing as the procedure arrives.
     * Start is still gated — that is the test three cases above.
     */
    it("keeps the bot-fill toggle while the engine has not published the distribution", () => {
      mocks.podState.packDistribution = null;
      render(<DraftPodLobby onLeave={vi.fn()} />);

      expect(screen.queryByText("Fill empty seats with bots")).toBeInTheDocument();
    });
  });
});
