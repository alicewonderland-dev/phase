/**
 * Draft Pod Store — UI state for P2P draft pod lobby management.
 *
 * This store manages pod-specific UI state that augments the
 * `multiplayerDraftStore` (which handles the adapter lifecycle,
 * draft picks, and deckbuilding). The pod store tracks:
 *
 * - Pod configuration (set, draft type, pod size)
 * - Bot-fill state (which empty seats to fill with bots on start)
 * - Lobby readiness and host controls
 *
 * The `multiplayerDraftStore` remains the source of truth for
 * adapter state, seat views, and draft phase. This store provides
 * the orchestration layer for the lobby UI.
 */

import { create } from "zustand";

import { DraftAdapter, distinctJoined, setPackSequence, type CubeDraftSettings, type DraftProcedure, type PoolInput, type SetPackSequence, type TournamentFormat, type PodPolicy } from "../adapter/draft-adapter";
import type { DraftPackChoice } from "./draftStore";
import type { DraftPodHostConfig } from "../adapter/draftPodHostAdapter";
import type { DraftPodGuestConfig } from "../adapter/draftPodGuestAdapter";
import {
  clearActiveDraftPodIfCurrent,
  inspectActiveDraftPod,
  loadDraftHostSession,
  persistedDraftHostSessionState,
} from "../services/draftPersistence";
import { useMultiplayerDraftStore } from "./multiplayerDraftStore";
import type { DraftKind } from "../components/draft/draftKind";

// ── Types ──────────────────────────────────────────────────────────────

export type PoolMode = "set" | "cube";

/** Result of one host-recovery probe. Page entry owns the route policy. */
export type HostedPodResumeOutcome = "resumed" | "absent" | "terminal" | "invalid" | "superseded";

export interface CubeForm {
  cubeName: string;
  cubeListText: string;
  settings: CubeDraftSettings;
}

export interface PodConfig {
  /**
   * The set filling each booster, in the order the host arranged them. One
   * entry per pack the pod opens; the same set may fill several. Empty until
   * the host picks, and a Cube pod leaves it empty — its pool is `cubeForm`.
   */
  packs: DraftPackChoice[];
  /**
   * Display label for the whole pool. Mirrors the engine's own source label
   * (`DraftSource::set_code`), which joins the DISTINCT set codes in
   * first-appearance order, so a mixed pod reads as "ISD+DKA+AVR".
   */
  setCode: string;
  setName: string;
  kind: DraftKind;
  podSize: number;
  tournamentFormat: TournamentFormat;
  podPolicy: PodPolicy;
}

interface DraftPodState {
  /** Pod configuration selected by host before creating the pod. */
  config: PodConfig;
  /** Whether bot-fill is enabled (fill remaining seats with bots on start). */
  botFillEnabled: boolean;
  /** Host display name for the local player. */
  hostDisplayName: string;
  /** Join code entered by guest. */
  joinCode: string;
  /** Guest display name. */
  guestDisplayName: string;
  /** Which pool source the host is configuring: a Set pool or a custom Cube list. */
  poolMode: PoolMode;
  /** Cube form state (cube name + list text + settings); null when poolMode === "set". */
  cubeForm: CubeForm | null;
  /** Set pool JSON loaded from draft-pools.json. Set-mode cache only — unused in cube mode. */
  setPoolJson: string | null;
  /** Loading state while fetching set pool data. */
  loadingPool: boolean;
  /** Error from pool loading or pod creation. */
  configError: string | null;
  /**
   * CR 903.13a + CR 800.1: the kind's engine-published seat floor
   * (`DraftProcedure.min_pod_size`), cached for the lobby's Start gate.
   *
   * A CACHE of an engine value, never a client derivation, and deliberately
   * outside `PodConfig` — that is host INTENT, is persisted, and is rewritten
   * by `normalizePodConfig`, none of which is true of a published floor.
   * `null` until loaded and after `reset()`, and `null` is fail-CLOSED: the
   * reducer is the authority, so a stale or absent client value can never
   * admit an illegal pod, only refuse a legal one until the engine answers.
   */
  minPodSize: number | null;
  /**
   * The kind's engine-published booster count (`DraftProcedure.packs_per_player`),
   * cached alongside `minPodSize` and on the same terms: a copy of an engine
   * value, never a client derivation. It fixes how many sets the host arranges,
   * so a Sealed pod asks for six and a draft pod for three without the page
   * knowing either number.
   *
   * `null` until loaded and after `reset()`. Fail-CLOSED like the seat floor:
   * with no answer yet the selector cannot be filled in, and the engine still
   * refuses a sequence longer than the kind opens regardless of this cache.
   */
  packsPerPlayer: number | null;
}

interface DraftPodActions {
  /** Update pod configuration fields. */
  setConfig: (partial: Partial<PodConfig>) => void;
  /** Enter pod setup for `kind`, adopting the ENGINE's per-kind table default for
   *  pod size (`DraftProcedure.pod_size`) rather than re-deriving one in the client.
   *  The host may still override it with the pod-size selector before creating. */
  enterKind: (kind: DraftKind) => Promise<void>;
  /** Toggle bot-fill on/off. */
  toggleBotFill: () => void;
  /** Set host display name. */
  setHostDisplayName: (name: string) => void;
  /** Set guest display name. */
  setGuestDisplayName: (name: string) => void;
  /** Set join code for guest. */
  setJoinCode: (code: string) => void;
  /**
   * Refresh the cached engine procedure axes for the currently selected kind.
   *
   * `setConfig({ kind })` — what the kind radios call — records the host's
   * intent but publishes nothing, so the cached axes would otherwise describe
   * whichever kind was loaded last. Unlike `enterKind` this does NOT adopt the
   * kind's default pod size: the host may already have chosen one, and
   * switching kinds must not silently discard it.
   */
  refreshProcedure: () => Promise<void>;
  /** Switch between Set-pool and Cube-list pool modes. */
  setPoolMode: (mode: PoolMode) => void;
  /** Set the cube form (name + list text + settings) for cube-mode host setup. */
  setCubeForm: (form: CubeForm | null) => void;
  /** Load the set pool data and create a new pod as host. */
  createPod: () => Promise<void>;
  /** Join an existing pod as guest. */
  joinPod: () => Promise<void>;
  /** Resume the active hosted pod from local persistence. */
  resumeHostedPod: (options?: { silent?: boolean; routeToken?: number; signal?: AbortSignal }) => Promise<HostedPodResumeOutcome>;
  /** Host: start the draft (delegates to multiplayerDraftStore). */
  startDraft: () => Promise<void>;
  /** Reset pod store state. */
  reset: () => void;
}

// ── Initial state ──────────────────────────────────────────────────────

const initialState: DraftPodState = {
  config: {
    packs: [],
    setCode: "",
    setName: "",
    kind: "Premier",
    podSize: 8,
    tournamentFormat: "Swiss",
    podPolicy: "Competitive",
  },
  botFillEnabled: true,
  hostDisplayName: "",
  guestDisplayName: "",
  joinCode: "",
  poolMode: "set",
  cubeForm: null,
  setPoolJson: null,
  loadingPool: false,
  configError: null,
  minPodSize: null,
  packsPerPlayer: null,
};

/**
 * The pack list a persisted pod was configured with.
 *
 * Only the live `SetPackSequence` spelling carries one. A pod persisted before
 * multi-set pods existed holds a single serialized pool and no sequence at all;
 * draft-wasm still starts it (every booster from that one set), so resuming
 * must not fail — it just has no per-pack list to show, and returns empty.
 * Names are not persisted, so each entry is labelled by its own code.
 */
function persistedPodPacks(poolInput: PoolInput): DraftPackChoice[] {
  if (poolInput.type !== "Set") return [];
  const sequence = (poolInput.data as Partial<SetPackSequence>).sequence;
  if (!Array.isArray(sequence)) return [];
  return sequence.map((code) => ({ code, name: code }));
}

function normalizePodConfig(config: PodConfig): PodConfig {
  if (config.tournamentFormat === "SingleElimination") {
    return { ...config, podSize: 8 };
  }
  return config;
}

interface HostedPodResumeAttempt {
  routeToken: number;
  signal: AbortSignal | undefined;
  promise: Promise<HostedPodResumeOutcome>;
}

let resumeHostedPodAttempt: HostedPodResumeAttempt | null = null;

/**
 * Fetch `kind`'s engine-published procedure and cache the axes the lobby needs.
 *
 * CR 903.13a + CR 800.1: `min_pod_size` is the ENGINE's per-kind seat floor.
 * The client holds a copy so `DraftPodLobby` can gate its Start button without
 * a second wasm call; it never re-derives the value, and the reducer refuses a
 * below-floor pod regardless of what this cache says.
 */
async function loadProcedure(
  kind: DraftKind,
  set: (partial: Partial<DraftPodState>) => void,
): Promise<DraftProcedure> {
  const procedure = await new DraftAdapter().draftProcedure(kind);
  set({
    minPodSize: procedure.min_pod_size,
    packsPerPlayer: procedure.packs_per_player,
  });
  return procedure;
}

// ── Store ──────────────────────────────────────────────────────────────

export const useDraftPodStore = create<DraftPodState & DraftPodActions>()(
  (set, get) => ({
    ...initialState,

    setConfig: (partial) => {
      set((prev) => ({
        config: normalizePodConfig({ ...prev.config, ...partial }),
        poolMode: (partial.kind ?? prev.config.kind) === "Sealed" ? "set" : prev.poolMode,
        configError: null,
      }));
    },

    enterKind: async (kind) => {
      // Apply the kind first: it is the entry point's whole purpose and must not
      // depend on the wasm load succeeding. `setConfig` is the single authority for
      // normalization (`normalizePodConfig`) and the Sealed pool-mode rule.
      get().setConfig({ kind });
      try {
        const procedure = await loadProcedure(kind, set);
        get().setConfig({ podSize: procedure.pod_size });
      } catch (err) {
        set({ configError: err instanceof Error ? err.message : String(err) });
      }
    },

    refreshProcedure: async () => {
      const kind = get().config.kind;
      try {
        const procedure = await new DraftAdapter().draftProcedure(kind);
        // The host may have switched kinds while this was in flight; a late
        // answer for the previous kind would publish the wrong booster count
        // and seat floor, so drop it rather than write it.
        if (get().config.kind !== kind) return;
        set({
          minPodSize: procedure.min_pod_size,
          packsPerPlayer: procedure.packs_per_player,
        });
      } catch (err) {
        set({ configError: err instanceof Error ? err.message : String(err) });
      }
    },

    toggleBotFill: () => {
      set((prev) => ({ botFillEnabled: !prev.botFillEnabled }));
    },

    setHostDisplayName: (name) => {
      set({ hostDisplayName: name });
    },

    setGuestDisplayName: (name) => {
      set({ guestDisplayName: name });
    },

    setJoinCode: (code) => {
      set({ joinCode: code });
    },

    setPoolMode: (mode) => {
      set((prev) => ({
        poolMode: prev.config.kind === "Sealed" ? "set" : mode,
        configError: null,
      }));
    },

    setCubeForm: (form) => {
      set({ cubeForm: form, configError: null });
    },

    createPod: async () => {
      const { config, hostDisplayName, poolMode, cubeForm } = get();

      if (config.kind === "Sealed" && poolMode !== "set") {
        set({ configError: "Sealed pods require a set pool" });
        return;
      }

      if (!hostDisplayName.trim()) {
        set({ configError: "Enter a display name" });
        return;
      }

      // CR 903.13a + CR 800.1: cache the kind's seat floor for the lobby's
      // Start gate. Once, before the poolMode branch, because the set branch
      // returns before reaching the cube branch and both lead to the lobby.
      try {
        await loadProcedure(config.kind, set);
      } catch (err) {
        set({ configError: err instanceof Error ? err.message : String(err) });
      }

      if (poolMode === "set") {
        if (config.packs.length === 0) {
          set({ configError: "Select a set first" });
          return;
        }

        // No `configError: null`: every other writer above in this function
        // returns, so clearing here would only erase `loadProcedure`'s catch.
        set({ loadingPool: true });

        try {
          const resp = await fetch(__DRAFT_POOLS_URL__);
          if (!resp.ok) {
            throw new Error(`Failed to load draft pools: ${resp.status}`);
          }
          const allPools: Record<string, unknown> = await resp.json();
          const selection = setPackSequence(config.packs, allPools);

          set({ setPoolJson: JSON.stringify(selection), loadingPool: false });

          const persistenceId = crypto.randomUUID();
          const hostConfig: DraftPodHostConfig = {
            poolInput: { type: "Set", data: selection },
            kind: config.kind,
            podSize: config.podSize,
            hostDisplayName: hostDisplayName.trim(),
            tournamentFormat: config.tournamentFormat,
            podPolicy: config.podPolicy,
            persistenceId,
          };

          await useMultiplayerDraftStore.getState().hostDraft(hostConfig);
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          set({ configError: message, loadingPool: false });
        }
        return;
      }

      // Cube mode: skip the draft-pools.json fetch entirely; everything the
      // host needs lives on the cubeForm object.
      if (!cubeForm || !cubeForm.cubeListText.trim()) {
        set({ configError: "Paste a cube list first" });
        return;
      }
      if (!cubeForm.cubeName.trim()) {
        set({ configError: "Enter a cube name" });
        return;
      }

      try {
        const persistenceId = crypto.randomUUID();
        const hostConfig: DraftPodHostConfig = {
          poolInput: {
            type: "Cube",
            data: {
              cube_list_text: cubeForm.cubeListText,
              cube_name: cubeForm.cubeName,
              cube_draft_settings: cubeForm.settings,
            },
          },
          kind: config.kind,
          podSize: config.podSize,
          hostDisplayName: hostDisplayName.trim(),
          tournamentFormat: config.tournamentFormat,
          podPolicy: config.podPolicy,
          persistenceId,
        };

        await useMultiplayerDraftStore.getState().hostDraft(hostConfig);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        set({ configError: message });
      }
    },

    resumeHostedPod: async (options = {}) => {
      const routeToken = options.routeToken ?? 0;
      if (
        resumeHostedPodAttempt &&
        resumeHostedPodAttempt.routeToken === routeToken &&
        resumeHostedPodAttempt.signal === options.signal
      ) {
        return resumeHostedPodAttempt.promise;
      }

      const attempt: HostedPodResumeAttempt = {
        routeToken,
        signal: options.signal,
        promise: Promise.resolve("superseded"),
      };
      const isCurrentAttempt = () =>
        resumeHostedPodAttempt === attempt && !options.signal?.aborted;
      attempt.promise = (async (): Promise<HostedPodResumeOutcome> => {
        if (options.signal?.aborted) return "superseded";
        const active = inspectActiveDraftPod();
        if (active.type === "absent") {
          if (!options.silent) set({ configError: "No draft pod to resume" });
          return "absent";
        }
        if (active.type === "invalid") {
          if (active.capture) clearActiveDraftPodIfCurrent(active.capture);
          if (!options.silent) set({ configError: "Saved draft pod is invalid" });
          return "invalid";
        }
        const { meta, capture } = active;

        const activeDraft = useMultiplayerDraftStore.getState();
        if (
          activeDraft.role === "host" &&
          activeDraft.phase !== "idle" &&
          activeDraft.phase !== "error" &&
          activeDraft.roomCode === meta.roomCode
        ) {
          return "resumed";
        }

        const persisted = await loadDraftHostSession(meta.id);
        if (!isCurrentAttempt()) return "superseded";
        if (!persisted) {
          clearActiveDraftPodIfCurrent(capture);
          if (!options.silent) set({ configError: "Saved draft pod was not found" });
          return "invalid";
        }
        const sessionState = persistedDraftHostSessionState(persisted);
        if (
          persisted.persistenceId !== meta.id ||
          persisted.roomCode !== meta.roomCode ||
          sessionState !== "live"
        ) {
          // Release only the active locator. A terminal snapshot is retained as
          // local history; a corrupt one is unreachable after this exact-match
          // cleanup and cannot poison a replacement pod.
          clearActiveDraftPodIfCurrent(capture);
          if (!options.silent) {
            set({ configError: sessionState === "terminal" ? "Saved draft pod is complete" : "Saved draft pod is invalid" });
          }
          return sessionState === "terminal" ? "terminal" : "invalid";
        }

        // Branch on the persisted pool source: restore the matching UI
        // state (poolMode + cubeForm or setPoolJson cache) so a refresh
        // mid-pod lands the host back on the same tab they configured.
        if (persisted.poolInput.type === "Cube") {
          const cubeData = persisted.poolInput.data;
          set({
            config: {
              packs: [],
              setCode: "custom-cube",
              setName: cubeData.cube_name,
              kind: persisted.kind,
              podSize: persisted.podSize,
              tournamentFormat: persisted.tournamentFormat,
              podPolicy: persisted.podPolicy,
            },
            hostDisplayName: persisted.hostDisplayName,
            poolMode: "cube",
            cubeForm: {
              cubeName: cubeData.cube_name,
              cubeListText: cubeData.cube_list_text,
              settings: cubeData.cube_draft_settings,
            },
            setPoolJson: null,
            loadingPool: false,
            configError: null,
          });
        } else {
          // Restore the pack sequence the pod was configured with, so a host
          // who refreshes mid-lobby sees the sets they arranged rather than an
          // unlabelled pod. A snapshot in the pre-multi-set spelling has no
          // sequence to restore; its packs stay empty and the label falls back.
          const packs = persistedPodPacks(persisted.poolInput);
          set({
            config: {
              packs,
              setCode: distinctJoined(packs.map((pack) => pack.code), "+"),
              setName: packs.length > 0 ? distinctJoined(packs.map((pack) => pack.name), " · ") : "Draft Pod",
              kind: persisted.kind,
              podSize: persisted.podSize,
              tournamentFormat: persisted.tournamentFormat,
              podPolicy: persisted.podPolicy,
            },
            hostDisplayName: persisted.hostDisplayName,
            poolMode: "set",
            cubeForm: null,
            setPoolJson: JSON.stringify(persisted.poolInput.data),
            loadingPool: false,
            configError: null,
          });
        }

        // CR 903.13a + CR 800.1: the resumed lobby needs the floor too.
        try {
          await loadProcedure(persisted.kind, set);
        } catch (err) {
          set({ configError: err instanceof Error ? err.message : String(err) });
        }

        const hostConfig: DraftPodHostConfig = {
          poolInput: persisted.poolInput,
          kind: persisted.kind,
          podSize: persisted.podSize,
          hostDisplayName: persisted.hostDisplayName,
          tournamentFormat: persisted.tournamentFormat,
          podPolicy: persisted.podPolicy,
          persistenceId: persisted.persistenceId,
          preferredRoomCode: persisted.roomCode || undefined,
        };

        if (!isCurrentAttempt()) return "superseded";
        const hosted = await useMultiplayerDraftStore.getState().hostDraft({
          ...hostConfig,
          signal: options.signal,
        });
        if (!isCurrentAttempt()) return "superseded";
        return hosted ? "resumed" : "invalid";
      })();
      resumeHostedPodAttempt = attempt;

      try {
        return await attempt.promise;
      } finally {
        if (resumeHostedPodAttempt === attempt) resumeHostedPodAttempt = null;
      }
    },

    joinPod: async () => {
      const { joinCode, guestDisplayName } = get();

      if (!joinCode.trim()) {
        set({ configError: "Enter a room code" });
        return;
      }
      if (!guestDisplayName.trim()) {
        set({ configError: "Enter a display name" });
        return;
      }

      set({ configError: null });

      const guestConfig: DraftPodGuestConfig = {
        kind: "new",
        roomCode: joinCode.trim(),
        displayName: guestDisplayName.trim(),
      };

      try {
        await useMultiplayerDraftStore.getState().joinDraft(guestConfig);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        set({ configError: message });
      }
    },

    startDraft: async () => {
      await useMultiplayerDraftStore.getState().startDraft(get().botFillEnabled);
    },

    reset: () => {
      set(initialState);
    },
  }),
);
