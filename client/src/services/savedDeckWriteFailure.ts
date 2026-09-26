/**
 * Surface a refused saved-deck library write to the user. Precedent for a non-component toast:
 * `game/actionRejectionReporter.ts::reportStructuredActionRejection`. Precedent for a typed
 * per-case label map: `draftDeckAutosave.ts::autosaveSlotLabels`.
 */
import i18n from "i18next";
import {
  SavedDeckLibraryBusyError,
  type SavedDeckTxnFailure,
  type SavedDeckTxnSkipReason,
} from "./savedDeckTransaction";
import { useAppNotificationStore } from "../stores/appToastStore";

export type SavedDeckWriteAction =
  | "save"
  | "clone"
  | "import"
  | "delete"
  | "organize"
  | "updateFeeds"
  | "restore"
  | "applyCloud";

const titleByAction: Record<SavedDeckWriteAction, () => string> = {
  save: () => i18n.t("savedDeckLibraryBusy.title.save"),
  clone: () => i18n.t("savedDeckLibraryBusy.title.clone"),
  import: () => i18n.t("savedDeckLibraryBusy.title.import"),
  delete: () => i18n.t("savedDeckLibraryBusy.title.delete"),
  organize: () => i18n.t("savedDeckLibraryBusy.title.organize"),
  updateFeeds: () => i18n.t("savedDeckLibraryBusy.title.updateFeeds"),
  restore: () => i18n.t("savedDeckLibraryBusy.title.restore"),
  applyCloud: () => i18n.t("savedDeckLibraryBusy.title.applyCloud"),
};

// D1: a lock refusal/timeout or a stale local view can self-heal by retrying; an IDB read or
// write failure will not, so it gets its own description rather than the "close other tabs" text.
const STORAGE_FAILURE_REASONS: ReadonlySet<SavedDeckTxnFailure> = new Set([
  "generation-unreadable",
  "generation-unpublished",
]);

export function savedDeckLibraryBusyDescription(): string {
  return i18n.t("savedDeckLibraryBusy.description");
}

export function savedDeckLibraryStorageFailureDescription(): string {
  return i18n.t("savedDeckLibraryStorageFailure.description");
}

function busyOrStorageDescription(reason: SavedDeckTxnFailure): string {
  return STORAGE_FAILURE_REASONS.has(reason)
    ? savedDeckLibraryStorageFailureDescription()
    : savedDeckLibraryBusyDescription();
}

export function notifySavedDeckLibraryBusy(action: SavedDeckWriteAction, reason?: SavedDeckTxnFailure): void {
  const description = reason !== undefined ? busyOrStorageDescription(reason) : savedDeckLibraryBusyDescription();
  useAppNotificationStore.getState().showNotification({ title: titleByAction[action](), description });
}

/** A background draft autosave (never rejects; `reason` comes from its skipped result) was not
 *  written. Unlike a user write, "no lock manager at all" gets its own description: nothing the
 *  user can do (close other tabs) will fix it. */
export function notifyDraftAutosaveSkipped(reason: SavedDeckTxnSkipReason): void {
  const description =
    reason === "lock-unavailable"
      ? i18n.t("savedDeckAutosaveUnavailable.description")
      : busyOrStorageDescription(reason);
  useAppNotificationStore.getState().showNotification({
    title: i18n.t("savedDeckLibraryBusy.title.autosaveDraft"),
    description,
  });
}

/** Run a user-initiated deck-library write; if it is refused, tell the user and resolve `{ ok: false }`. Other errors propagate. */
export async function attemptSavedDeckWrite<T>(
  action: SavedDeckWriteAction,
  write: () => Promise<T>,
): Promise<{ ok: true; value: T } | { ok: false }> {
  try {
    return { ok: true, value: await write() };
  } catch (error) {
    if (error instanceof SavedDeckLibraryBusyError) {
      notifySavedDeckLibraryBusy(action, error.reason);
      return { ok: false };
    }
    throw error;
  }
}
