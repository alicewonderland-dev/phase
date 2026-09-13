import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  DraftCardInstance,
  SeatPublicView,
  SharedStackPileView,
  SharedStackRefusal,
  SharedStackView,
} from "../../../adapter/draft-adapter";
import { WinstonPileTable } from "../WinstonPileTable";

// The image ladder is not what this surface is about, and resolving it would
// reach the Scryfall service. Same stub the pack-display tests use.
vi.mock("../../../hooks/useCardImage", () => ({
  useCardImage: () => ({ src: null, isLoading: false }),
}));

function card(id: string, name: string): DraftCardInstance {
  return {
    instance_id: id,
    name,
    set_code: "tst",
    collector_number: "1",
    rarity: "common",
    colors: ["U"],
    cmc: 2,
    type_line: "Instant",
  };
}

const seats: SeatPublicView[] = [
  {
    seat_index: 0,
    display_name: "Alice",
    is_bot: false,
    connected: true,
    has_submitted_deck: false,
    pick_status: "Pending",
    active_pack_count: 0,
    face_up_draft_cards: [],
  },
  {
    seat_index: 1,
    display_name: "Bob",
    is_bot: false,
    connected: true,
    has_submitted_deck: false,
    pick_status: "Waiting",
    active_pack_count: 0,
    face_up_draft_cards: [],
  },
];

/** A pile exactly as the engine publishes one: counts, a revealed PREFIX, and a
 *  verdict per decision. `null` refusal means legal. */
function pile(
  index: number,
  total: number,
  revealed: DraftCardInstance[],
  take: SharedStackRefusal | null,
  decline: SharedStackRefusal | null,
): SharedStackPileView {
  return {
    index,
    total,
    revealed,
    legality: [
      { decision: "Take", refusal: take },
      { decision: "Decline", refusal: decline },
    ],
  };
}

/** The live turn as the engine publishes it. Every field here is PUBLIC —
 *  including `active_pile`, which every viewer receives — so the same object
 *  serves the active seat and an onlooker; what distinguishes them is the
 *  `viewerSeat` the table is rendered with, plus the `revealed` prefixes, which
 *  are the one viewer-scoped thing. `active_seat` is 0 (Alice). */
function activeTurn(piles: SharedStackPileView[], activePile: number): SharedStackView {
  return {
    main_stack_remaining: 17,
    total_cards: 23,
    active_seat: 0,
    active_pile: activePile,
    piles,
    decisions: 4,
  };
}

function renderTable(
  sharedStack: SharedStackView,
  overrides: {
    onDecide?: (pile: number, decision: string) => void;
    interactionLocked?: boolean;
    playFirstChooser?: number | null;
    /** Defaults to seat 0, which `activeTurn` makes the ACTIVE seat. */
    viewerSeat?: number | null;
  } = {},
) {
  const onDecide = overrides.onDecide ?? vi.fn();
  const rendered = render(
    <WinstonPileTable
      sharedStack={sharedStack}
      seats={seats}
      viewerSeat={overrides.viewerSeat === undefined ? 0 : overrides.viewerSeat}
      playFirstChooser={overrides.playFirstChooser ?? null}
      interactionLocked={overrides.interactionLocked ?? false}
      onDecide={onDecide}
    />,
  );
  return { ...rendered, onDecide };
}

function decisionButton(pileIndex: number, decision: "Take" | "Decline"): HTMLButtonElement {
  const host = document.querySelector(`[data-winston-pile="${pileIndex}"]`);
  expect(host).not.toBeNull();
  const button = host!.querySelector<HTMLButtonElement>(`[data-winston-decision="${decision}"]`);
  expect(button).not.toBeNull();
  return button!;
}

describe("WinstonPileTable", () => {
  afterEach(cleanup);

  it("enables Take exactly when the engine publishes no refusal for it", () => {
    // Paired positive: the same fixture shape, differing ONLY in the published
    // verdict, so neither arm can pass by accident. Nothing about the counts
    // changes between them — a client that derived legality from `total` would
    // answer identically in both.
    const legal = renderTable(activeTurn([pile(0, 3, [card("c1", "Ponder")], null, null)], 0));
    expect(decisionButton(0, "Take")).toBeEnabled();
    cleanup();

    renderTable(activeTurn([pile(0, 3, [card("c1", "Ponder")], "PileEmpty", null)], 0));
    expect(decisionButton(0, "Take")).toBeDisabled();
    // The engine's OWN reason is rendered, not one reinvented here.
    expect(screen.getByText("This pile is empty, so there is nothing to take.")).toBeInTheDocument();
    expect(decisionButton(0, "Take")).toHaveAttribute(
      "title",
      "This pile is empty, so there is nothing to take.",
    );
    expect(legal).toBeTruthy();
  });

  it("disables Decline on a published refusal and says why taking is mandatory", () => {
    renderTable(activeTurn([pile(0, 2, [card("c1", "Ponder")], null, "NoGuaranteedCard")], 0));

    expect(decisionButton(0, "Take")).toBeEnabled();
    expect(decisionButton(0, "Decline")).toBeDisabled();
    expect(
      screen.getByText(
        "Declining can no longer leave you a card this turn, so taking this pile is your only legal move.",
      ),
    ).toBeInTheDocument();
  });

  it("refuses to enable a decision the engine published no verdict for", () => {
    // Hostile fixture: a legality vector that does not mention `Take` at all.
    // "No verdict" must not read as permission.
    const unverdicted: SharedStackPileView = {
      index: 0,
      total: 4,
      revealed: [],
      legality: [{ decision: "Decline", refusal: null }],
    };
    renderTable(activeTurn([unverdicted], 0));

    expect(decisionButton(0, "Take")).toBeDisabled();
    expect(screen.getByText("The draft published no verdict for this decision.")).toBeInTheDocument();
    expect(decisionButton(0, "Decline")).toBeEnabled();
  });

  it("dispatches the engine's own pile index, and only for the pile being decided", () => {
    const onDecide = vi.fn();
    renderTable(
      activeTurn(
        [
          pile(0, 1, [], null, null),
          pile(1, 5, [card("c2", "Opt")], null, null),
          pile(2, 2, [], "PileNotActive", "PileNotActive"),
        ],
        1,
      ),
      { onDecide },
    );

    // Only the cursor pile carries controls.
    expect(document.querySelectorAll("[data-winston-decision]")).toHaveLength(2);
    fireEvent.click(decisionButton(1, "Take"));
    expect(onDecide).toHaveBeenCalledWith(1, "Take");

    fireEvent.click(decisionButton(1, "Decline"));
    expect(onDecide).toHaveBeenCalledWith(1, "Decline");
  });

  it("shows nothing face up and offers no control to a seat whose turn it is not", () => {
    // Exactly what `filter_for_player` publishes to the NON-ACTIVE seat, and the
    // discriminating detail is what it does NOT withhold: every `revealed` is
    // empty, but the counts AND `active_pile` are published in full, identical
    // to the active seat's own projection. The onlooker is seat 1; `active_seat`
    // is 0. A `null` cursor here would be fiction — the engine publishes it.
    const spectatingSeat: SharedStackView = {
      main_stack_remaining: 17,
      total_cards: 23,
      active_seat: 0,
      active_pile: 1,
      piles: [pile(0, 3, [], null, null), pile(1, 1, [], null, null), pile(2, 4, [], null, null)],
      decisions: 4,
    };
    renderTable(spectatingSeat, { viewerSeat: 1 });

    // Reach guard: the table rendered, and rendered the public counts.
    expect(screen.getByText("17 cards face down in the main stack")).toBeInTheDocument();
    expect(screen.getByText("23 cards left in the draft")).toBeInTheDocument();
    // Whose turn it is comes from `active_seat` compared against `viewerSeat`,
    // NOT from `active_pile` — which is non-null here precisely so that a
    // regression to `active_pile !== null` reds this line.
    expect(screen.getByText("Alice is deciding")).toBeInTheDocument();
    expect(screen.getByText("The piles stay face down until it is your turn.")).toBeInTheDocument();

    // The cursor IS rendered to the onlooker: which pile is being handled is
    // open information at a physical table. This is the positive half — without
    // it, the "no controls" assertion below could pass on a table that simply
    // failed to identify the cursor at all.
    expect(document.querySelector("[data-winston-pile-active='true']"))
      .toBe(document.querySelector('[data-winston-pile="1"]'));

    // And the secret half: nothing face up, and no control — even though every
    // published verdict on this view says the decision is legal FOR THE ACTIVE
    // SEAT, and even though this viewer can see which pile that seat is on.
    expect(document.querySelectorAll("[data-winston-revealed-card]")).toHaveLength(0);
    expect(document.querySelectorAll("[data-winston-decision]")).toHaveLength(0);
  });

  /**
   * The paired positive for the test above, differing ONLY in `viewerSeat`: the
   * SAME published projection, rendered for the active seat, does offer the
   * controls. That is what proves the suppression above is a seat comparison
   * and not a table that never renders controls at all.
   */
  it("offers the cursor pile's controls to the seat whose turn it is", () => {
    const sameProjection: SharedStackView = {
      main_stack_remaining: 17,
      total_cards: 23,
      active_seat: 0,
      active_pile: 1,
      piles: [pile(0, 3, [], null, null), pile(1, 1, [], null, null), pile(2, 4, [], null, null)],
      decisions: 4,
    };
    renderTable(sameProjection, { viewerSeat: 0 });

    expect(screen.getByText("Your turn — pile 2")).toBeInTheDocument();
    expect(document.querySelectorAll("[data-winston-decision]")).toHaveLength(2);
    expect(decisionButton(1, "Take")).toBeEnabled();
  });

  /**
   * A viewer with no assigned seat is NOT the active seat. `null` must fall to
   * the no-controls side rather than comparing equal to anything.
   */
  it("offers no control to a viewer with no assigned seat", () => {
    renderTable(activeTurn([pile(0, 3, [card("c1", "Ponder")], null, null)], 0), {
      viewerSeat: null,
    });

    expect(document.querySelectorAll("[data-winston-decision]")).toHaveLength(0);
    expect(screen.getByText("Alice is deciding")).toBeInTheDocument();
  });

  it("renders the revealed prefix verbatim and the rest as a count", () => {
    renderTable(
      activeTurn([pile(0, 4, [card("c1", "Ponder"), card("c2", "Opt")], null, null)], 0),
    );

    expect(document.querySelectorAll("[data-winston-revealed-card]")).toHaveLength(2);
    expect(screen.getByText("Ponder")).toBeInTheDocument();
    expect(screen.getByText("Opt")).toBeInTheDocument();
    // The pile is taller than the prefix, and the remainder is a HEIGHT only.
    expect(screen.getByText("4 cards")).toBeInTheDocument();
    expect(screen.getByText("Your turn — pile 1")).toBeInTheDocument();
  });

  it("locks both controls while a decision is in flight, claiming no refusal", () => {
    renderTable(activeTurn([pile(0, 3, [card("c1", "Ponder")], null, null)], 0), {
      interactionLocked: true,
    });

    expect(decisionButton(0, "Take")).toBeDisabled();
    expect(decisionButton(0, "Decline")).toBeDisabled();
    // A lock is not a legality statement, so no engine reason is attached.
    expect(decisionButton(0, "Take")).not.toHaveAttribute("title");
  });

  it("states the play-first choice as an instruction, never as a control", () => {
    renderTable(activeTurn([pile(0, 3, [], null, null)], 0), { playFirstChooser: 1 });

    const advisory = screen.getByText("Bob chooses who plays first in the games after the draft.");
    expect(advisory).toBeInTheDocument();
    expect(advisory.tagName).toBe("P");
    expect(advisory.querySelector("button")).toBeNull();
  });

  it("omits the play-first line when the engine published no chooser", () => {
    renderTable(activeTurn([pile(0, 3, [], null, null)], 0), { playFirstChooser: null });

    expect(document.querySelector("[data-winston-play-first]")).toBeNull();
  });
});
