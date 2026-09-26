/**
 * Surface a refused saved-deck library write to the user. Precedent for a non-component toast:
 * `game/actionRejectionReporter.ts::reportStructuredActionRejection`. Precedent for a typed
 * per-case label map: `draftDeckAutosave.ts::autosaveSlotLabels`.
 */
import i18n from "i18next";
import { SavedDeckLibraryBusyError, type SavedDeckTxnFailure } from "./savedDeckTransaction";
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

export function notifySavedDeckLibraryBusy(action: SavedDeckWriteAction, reason?: SavedDeckTxnFailure): void {
  const description =
    reason !== undefined && STORAGE_FAILURE_REASONS.has(reason)
      ? savedDeckLibraryStorageFailureDescription()
      : savedDeckLibraryBusyDescription();
  useAppNotificationStore.getState().showNotification({ title: titleByAction[action](), description });
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
