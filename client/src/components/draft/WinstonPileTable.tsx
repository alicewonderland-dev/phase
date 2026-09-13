/**
 * Winston Pile Table — the drafting-phase surface for a `SharedStackPiles` draft.
 *
 * DISPLAY LAYER, and unusually strictly so. Every fact on this screen is read from
 * `DraftPlayerView.shared_stack` exactly as the engine published it:
 *
 *   * whether a control is available comes from `pile.legality[].refusal`, the
 *     engine's single legality authority (`shared_stack::refusal_for`), never from
 *     a pile's `total` or from `main_stack_remaining`. There is no arithmetic over
 *     pile sizes anywhere in this file, because any such arithmetic would be a
 *     SECOND authority for a question the reducer already answers — and the two
 *     could then disagree, which is the one failure mode the published vector
 *     exists to make impossible;
 *   * whose turn it is comes from `active_seat`;
 *   * WHICH pile this seat is deciding on comes from `active_pile`, which the
 *     engine publishes only to the active seat, so `active_pile === null` IS the
 *     "not your turn" state and needs no seat comparison here;
 *   * what may be shown face up comes from `pile.revealed`, rendered verbatim. A
 *     pile whose `revealed` is shorter than its `total` is showing precisely what
 *     this seat is entitled to see this turn: the prefix it has looked at. The
 *     remainder — including the card a decline just added — is face down, and this
 *     component has no representation for it beyond the count.
 *
 * `play_first_chooser` is rendered as an INSTRUCTION to the players and never as a
 * control: the engine does not enforce the choice (it is advisory), so offering a
 * button would claim an authority no reducer backs.
 */

import { useTranslation } from "react-i18next";

import type {
  DraftCardInstance,
  SeatPublicView,
  SharedStackPileDecision,
  SharedStackPileView,
  SharedStackRefusal,
  SharedStackView,
} from "../../adapter/draft-adapter";
import type { CardHoverInfo } from "../card/CardPreview";
import { menuButtonClass } from "../menu/buttonStyles";
import { useDraftCardFace } from "./DraftCardFace.tsx";

export interface WinstonPileTableProps {
  /** The engine's projection of the live turn FOR THIS VIEWER. */
  sharedStack: SharedStackView;
  /** Seat list from the same view, for naming the seat whose turn it is. */
  seats: readonly SeatPublicView[];
  /** `DraftPlayerView.play_first_chooser` — advisory, rendered as a sentence. */
  playFirstChooser?: number | null;
  /** A decision is in flight, or the pod is paused. Not a legality statement: it
   *  suppresses a second dispatch and carries no refusal reason. */
  interactionLocked: boolean;
  onDecide: (pile: number, decision: SharedStackPileDecision) => void;
  onCardHover?: (info: CardHoverInfo | null) => void;
}

/**
 * The engine's verdict for one decision on one pile.
 *
 * Looked up BY `decision` rather than by position: the published vector is built
 * from `SharedStackPileDecision::ALL`, so a widened axis must not silently shift
 * which verdict a button reads. `undefined` — no verdict published for this
 * decision at all — is deliberately distinct from `null` (published, and legal).
 */
function verdictFor(
  pile: SharedStackPileView,
  decision: SharedStackPileDecision,
): SharedStackRefusal | null | undefined {
  const entry = pile.legality.find((candidate) => candidate.decision === decision);
  return entry === undefined ? undefined : entry.refusal;
}

// ── Revealed card ───────────────────────────────────────────────────────

function RevealedCard({
  card,
  onCardHover,
}: {
  card: DraftCardInstance;
  onCardHover?: (info: CardHoverInfo | null) => void;
}) {
  const sourcePrinting = { setCode: card.set_code, collectorNumber: card.collector_number };
  const { src, isLoading, displayName } = useDraftCardFace(card.name, sourcePrinting);
  const preview = { name: card.name, sourcePrinting };

  return (
    <div
      data-winston-revealed-card={card.instance_id}
      className="shrink-0 overflow-hidden rounded-md ring-1 ring-white/15"
      style={{ width: 88, aspectRatio: "488 / 680" }}
      onMouseEnter={() => onCardHover?.(preview)}
      onMouseLeave={() => onCardHover?.(null)}
    >
      {isLoading || src === null ? (
        <span className="flex h-full items-center justify-center bg-white/5 px-1 text-center text-[10px] leading-tight text-white/60">
          {card.name}
        </span>
      ) : (
        <img src={src} alt={displayName} draggable={false} className="h-full w-full object-contain" />
      )}
    </div>
  );
}

// ── One pile ────────────────────────────────────────────────────────────

function Pile({
  pile,
  isActive,
  interactionLocked,
  onDecide,
  onCardHover,
}: {
  pile: SharedStackPileView;
  isActive: boolean;
  interactionLocked: boolean;
  onDecide: (pile: number, decision: SharedStackPileDecision) => void;
  onCardHover?: (info: CardHoverInfo | null) => void;
}) {
  const { t } = useTranslation("draft");
  // Piles are 0-indexed on the wire and 1-indexed in copy. A display offset, and
  // the only number this component derives at all.
  const label = pile.index + 1;

  const decisionButton = (decision: SharedStackPileDecision, tone: "emerald" | "neutral") => {
    const refusal = verdictFor(pile, decision);
    // Disabled unless the engine published "legal". An unpublished verdict is NOT
    // treated as permission: no verdict, no control.
    const refused = refusal !== null;
    const reason = refusal === undefined
      ? t("winston.refusalUnpublished")
      : refusal === null ? undefined : t(`winston.refusal.${refusal}`);
    const disabled = refused || interactionLocked;
    return (
      <button
        type="button"
        data-winston-decision={decision}
        disabled={disabled}
        title={reason}
        aria-label={t(decision === "Take" ? "winston.takeAria" : "winston.declineAria", { index: label })}
        aria-describedby={reason === undefined ? undefined : `winston-refusal-${pile.index}-${decision}`}
        onClick={() => onDecide(pile.index, decision)}
        className={menuButtonClass({ tone, size: "sm", disabled, className: "flex-1" })}
      >
        {t(decision === "Take" ? "winston.take" : "winston.decline")}
      </button>
    );
  };

  const refusalNotes = (["Take", "Decline"] as const).flatMap((decision) => {
    const refusal = verdictFor(pile, decision);
    if (refusal === null) return [];
    return [{
      decision,
      text: refusal === undefined ? t("winston.refusalUnpublished") : t(`winston.refusal.${refusal}`),
    }];
  });

  return (
    <div
      data-winston-pile={pile.index}
      data-winston-pile-active={isActive ? "true" : "false"}
      className={`flex min-w-0 flex-col gap-3 rounded-[16px] border p-3 ${
        isActive
          ? "border-amber-300/40 bg-amber-400/[0.06] shadow-[inset_0_-1px_0_rgba(0,0,0,0.28)]"
          : "border-hairline bg-white/[0.035]"
      }`}
    >
      <div className="flex min-w-0 items-baseline justify-between gap-2">
        <span className="truncate text-[0.68rem] font-semibold uppercase tracking-[0.18em] text-white/60">
          {t("winston.pileLabel", { index: label })}
        </span>
        <span data-winston-pile-total className="shrink-0 text-xs tabular-nums text-white/45">
          {t("winston.pileTotal", { count: pile.total })}
        </span>
      </div>

      {isActive && (
        <span className="text-[0.6rem] font-semibold uppercase tracking-[0.18em] text-amber-200/80">
          {t("winston.deciding")}
        </span>
      )}

      <div className="flex min-w-0 items-start gap-2 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        {/* The face-down remainder. A HEIGHT, never contents: the stack's order is
            published to nobody, and a pile's unlooked-at cards to nobody either. */}
        <div
          data-winston-pile-facedown
          aria-label={t("winston.pileFaceDown")}
          className="relative flex shrink-0 items-center justify-center rounded-md border border-white/12 bg-slate-900/80"
          style={{ width: 44, aspectRatio: "488 / 680" }}
        >
          <span className="text-sm font-semibold tabular-nums text-white/70">{pile.total}</span>
        </div>
        {pile.revealed.map((card) => (
          <RevealedCard key={card.instance_id} card={card} onCardHover={onCardHover} />
        ))}
        {isActive && pile.revealed.length === 0 && (
          <span className="self-center text-xs text-white/35">{t("winston.revealedNone")}</span>
        )}
      </div>

      {isActive && (
        <div className="flex flex-col gap-1.5">
          <div className="flex gap-2">
            {decisionButton("Take", "emerald")}
            {decisionButton("Decline", "neutral")}
          </div>
          {refusalNotes.map((note) => (
            <p
              key={note.decision}
              id={`winston-refusal-${pile.index}-${note.decision}`}
              className="text-xs text-amber-200/70"
            >
              {note.text}
            </p>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Component ───────────────────────────────────────────────────────────

export function WinstonPileTable({
  sharedStack,
  seats,
  playFirstChooser,
  interactionLocked,
  onDecide,
  onCardHover,
}: WinstonPileTableProps) {
  const { t } = useTranslation("draft");
  const { active_pile, active_seat, main_stack_remaining, total_cards, piles } = sharedStack;

  const seatName = (seat: number) =>
    seats.find((entry) => entry.seat_index === seat)?.display_name
    ?? t("winston.seatFallback", { index: seat + 1 });

  // `active_pile` is published ONLY to the active seat, so this is the engine's own
  // viewer-scoped answer to "is it my turn" rather than a seat comparison made here.
  const yourTurn = active_pile !== null;
  // A percentage of two published counts, for a bar width only. It answers no
  // question about any control.
  const faceDownPercent = total_cards === 0 ? 0 : (main_stack_remaining / total_cards) * 100;

  return (
    <section
      data-winston-pile-table
      aria-label={t("winston.heading")}
      className="mb-2 flex w-full min-w-0 flex-col gap-2"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-[16px] border border-hairline bg-white/[0.035] px-4 py-2 shadow-[inset_0_-1px_0_rgba(0,0,0,0.28)]">
        <span data-winston-turn role="status" aria-live="polite" className="text-sm font-semibold text-fg">
          {yourTurn
            ? t("winston.yourTurn", { index: active_pile + 1 })
            : t("winston.otherSeatTurn", { name: seatName(active_seat) })}
        </span>
        <span className="text-xs text-white/45">
          {t("winston.mainStackRemaining", { count: main_stack_remaining })}
        </span>
        <span className="ml-auto shrink-0 text-xs tabular-nums text-white/45">
          {t("winston.cardsLeft", { count: total_cards })}
        </span>
      </div>

      <div
        role="img"
        aria-label={t("winston.stackShare")}
        className="h-1.5 w-full overflow-hidden rounded-full bg-white/5"
      >
        <div
          data-winston-stack-share
          className="h-full rounded-full bg-amber-400/50 transition-[width] duration-200"
          style={{ width: `${faceDownPercent}%` }}
        />
      </div>

      <p className="text-xs text-white/40">
        {yourTurn ? t("winston.turnHint") : t("winston.hiddenPiles")}
      </p>

      <div className="grid gap-2 grid-cols-[repeat(auto-fit,minmax(min(100%,14rem),1fr))]">
        {piles.map((pile) => (
          <Pile
            key={pile.index}
            pile={pile}
            isActive={pile.index === active_pile}
            interactionLocked={interactionLocked}
            onDecide={onDecide}
            onCardHover={onCardHover}
          />
        ))}
      </div>

      {playFirstChooser !== null && playFirstChooser !== undefined && (
        <p data-winston-play-first className="text-xs text-white/40">
          {t("winston.playFirstChooser", { name: seatName(playFirstChooser) })}
        </p>
      )}
    </section>
  );
}
