import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Real `multiplayerDraftStore` and real `DraftPodGuestAdapter`; the PeerJS
 * room join and the `P2PDraftGuest` protocol object are replaced. A gated
 * `joinRoom` ignores its abort signal until released, as
 * `connection.ts::joinRoom` does while it awaits `getPeerConfig()`.
 */
type JoinResult = {
  conn: { peer: string };
  peer: { id: string; destroy: () => void };
  closeConn: () => void;
  destroyPeer: () => void;
};
const net = vi.hoisted(() => ({
  plan: [] as Array<(code: string, signal?: AbortSignal) => Promise<unknown>>,
  joinRoom: vi.fn(),
}));
vi.mock("../../network/connection", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../network/connection")>()),
  joinRoom: net.joinRoom,
}));

type GuestInstance = {
  emit: (event: unknown) => void;
  dispose: ReturnType<typeof vi.fn>;
  initialize: ReturnType<typeof vi.fn>;
};
const guests = vi.hoisted(() => ({ instances: [] as GuestInstance[], nextInitialize: [] as Array<() => Promise<void>> }));
vi.mock("../../adapter/p2p-draft-guest", () => ({
  P2PDraftGuest: vi.fn().mockImplementation(function () {
    let listener: ((event: unknown) => void) | null = null;
    const init = guests.nextInitialize.shift() ?? (async () => {});
    const instance = {
      onEvent: (l: (event: unknown) => void) => {
        listener = l;
        return () => {
          listener = null;
        };
      },
      initialize: vi.fn(init),
      leave: vi.fn(async () => {}),
      dispose: vi.fn(),
      get isRecoveryRevoked() {
        return false;
      },
      view: null,
      seat: null,
      token: null,
    };
    guests.instances.push({ emit: (e) => listener?.(e), dispose: instance.dispose, initialize: instance.initialize });
    return instance;
  }),
}));

type HostInstance = { dispose: ReturnType<typeof vi.fn> };
const hosts = vi.hoisted(() => ({ instances: [] as HostInstance[], nextInitialize: [] as Array<() => Promise<void>> }));
vi.mock("../../adapter/draftPodHostAdapter", () => ({
  DraftPodHostAdapter: vi.fn().mockImplementation(function () {
    const init = hosts.nextInitialize.shift() ?? (async () => {});
    const instance = {
      onEvent: () => () => {},
      initialize: vi.fn(init),
      dispose: vi.fn(async () => {}),
      status: "lobby",
      roomCode: "HOSTD",
    };
    hosts.instances.push({ dispose: instance.dispose });
    return instance;
  }),
}));

const persist = vi.hoisted(() => ({
  inspectActiveDraftGuest: vi.fn((): unknown => ({ type: "absent" })),
  loadDraftGuestSession: vi.fn(),
  clearActiveDraftGuestIfCurrent: vi.fn(),
  clearActiveDraftPod: vi.fn(),
}));
vi.mock("../../services/draftPersistence", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../services/draftPersistence")>()),
  inspectActiveDraftGuest: persist.inspectActiveDraftGuest,
  loadDraftGuestSession: persist.loadDraftGuestSession,
  clearActiveDraftGuestIfCurrent: persist.clearActiveDraftGuestIfCurrent,
  clearActiveDraftPod: persist.clearActiveDraftPod,
}));

import { useMultiplayerDraftStore } from "../multiplayerDraftStore";
import { useConnectivityStore } from "../connectivityStore";

function joinResult(code: string): JoinResult {
  return {
    conn: { peer: `phase2-${code}` },
    peer: { id: `peer-${code}`, destroy: vi.fn() },
    closeConn: vi.fn(),
    destroyPeer: vi.fn(),
  };
}

/** A room join that settles only when released, and only then looks at its abort signal. */
function gateNextJoin(): { release: () => void } {
  const gate = { release: () => {} };
  net.plan.push((code, signal) => new Promise((resolve, reject) => {
    gate.release = () => {
      if (signal?.aborted) reject(new DOMException("Aborted", "AbortError"));
      else resolve(joinResult(code));
    };
  }));
  return gate;
}

const hostConfig = {
  poolInput: { type: "Set", data: { pools: [{ code: "TST" }], sequence: ["TST"] } },
  kind: "Premier",
  podSize: 8,
  hostDisplayName: "Host",
  tournamentFormat: "Swiss",
  podPolicy: "Competitive",
} as unknown as Parameters<ReturnType<typeof useMultiplayerDraftStore.getState>["hostDraft"]>[0];

describe("session-open outcome of a superseded attempt", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    net.plan = [];
    guests.instances = [];
    guests.nextInitialize = [];
    hosts.instances = [];
    hosts.nextInitialize = [];
    persist.inspectActiveDraftGuest.mockReturnValue({ type: "absent" });
    net.joinRoom.mockImplementation((code: string, signal?: AbortSignal) => {
      const next = net.plan.shift();
      return next ? next(code, signal) : Promise.resolve(joinResult(code));
    });
  });

  afterEach(async () => {
    useConnectivityStore.setState({ forcedOffline: false, browserOnline: true });
    await useMultiplayerDraftStore.getState().leave(true);
  });

  it("reports its own failure as failed", async () => {
    net.plan.push(async () => {
      throw new Error("Failed to connect: Could not connect to peer OWN");
    });
    const outcome = await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "OWN", displayName: "A" });
    expect(outcome).toBe("failed");
    expect(useMultiplayerDraftStore.getState()).toMatchObject({ role: "guest", phase: "error", error: "Failed to connect: Could not connect to peer OWN" });
  });

  it.each([
    ["a newer join that failed", "failed"],
    ["a newer join that was kicked", "kicked"],
    ["a newer join that is live", "live"],
  ] as const)("reports superseded, keeping the newer session's role, phase and error, when it settles after %s", async (_label, newerShape) => {
    const stale = gateNextJoin();
    const staleJoin = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "STALE", displayName: "A" });
    await vi.waitFor(() => expect(net.joinRoom).toHaveBeenCalledTimes(1));

    if (newerShape === "failed") {
      net.plan.push(async () => {
        throw new Error("Failed to connect: Could not connect to peer NEWER");
      });
    }
    await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWER", displayName: "B" });
    if (newerShape === "kicked") guests.instances[0]!.emit({ type: "kicked", reason: "Kicked by host" });
    const before = useMultiplayerDraftStore.getState();
    const snapshot = { role: before.role, phase: before.phase, error: before.error };
    expect(snapshot.phase).toBe(newerShape === "failed" ? "error" : newerShape === "kicked" ? "kicked" : "lobby");

    stale.release();
    const outcome = await staleJoin;
    expect(outcome).toBe("superseded");
    const after = useMultiplayerDraftStore.getState();
    expect({ role: after.role, phase: after.phase, error: after.error }).toEqual(snapshot);
    for (const g of guests.instances) expect(g.dispose).not.toHaveBeenCalled();
  });

  it("reports superseded when its join rejects on the newer attempt's abort", async () => {
    net.plan.push((_code, signal) => new Promise((_resolve, reject) => {
      signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")), { once: true });
    }));
    const staleJoin = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "STALE", displayName: "A" });
    await vi.waitFor(() => expect(net.joinRoom).toHaveBeenCalledTimes(1));
    await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWER", displayName: "B" });
    expect(await staleJoin).toBe("superseded");
  });

  it("reports superseded when the session was left while it was pending", async () => {
    const stale = gateNextJoin();
    const staleJoin = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "STALE", displayName: "A" });
    await vi.waitFor(() => expect(net.joinRoom).toHaveBeenCalledTimes(1));
    await useMultiplayerDraftStore.getState().leave();
    stale.release();
    const outcome = await staleJoin;
    expect(outcome).toBe("superseded");
  });

  it.each(["resolves", "rejects"] as const)(
    "host: reports superseded when a newer join takes the session before its initialize %s",
    async (settlement) => {
      let settleHost!: () => void;
      hosts.nextInitialize.push(() => new Promise<void>((resolve, reject) => {
        settleHost = settlement === "resolves" ? resolve : () => reject(new Error("host signaling failed"));
      }));
      const hosting = useMultiplayerDraftStore.getState().hostDraft(hostConfig);
      await vi.waitFor(() => expect(hosts.instances).toHaveLength(1));
      net.plan.push(async () => {
        throw new Error("Failed to connect: Could not connect to peer NEWER");
      });
      await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWER", displayName: "B" });
      const before = useMultiplayerDraftStore.getState();
      const snapshot = { role: before.role, phase: before.phase, error: before.error };
      settleHost();
      const outcome = await hosting;
      expect(outcome).toBe("superseded");
      const after = useMultiplayerDraftStore.getState();
      expect({ role: after.role, phase: after.phase, error: after.error }).toEqual(snapshot);
    },
  );

  it("reports superseded when a newer attempt starts while it waits for the previous session's teardown", async () => {
    await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "HELD", displayName: "A" });
    const replacing = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "MIDDLE", displayName: "A" });
    const newest = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWEST", displayName: "A" });
    const outcome = await replacing;
    await newest;
    expect(outcome).toBe("superseded");
  });

  it("host: reports superseded when a newer attempt starts while it waits for the previous session's teardown", async () => {
    await useMultiplayerDraftStore.getState().hostDraft(hostConfig);
    const replacing = useMultiplayerDraftStore.getState().hostDraft(hostConfig);
    const newest = useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWEST", displayName: "A" });
    const outcome = await replacing;
    await newest;
    expect(outcome).toBe("superseded");
  });

  it("host: reports its own failure as failed", async () => {
    hosts.nextInitialize.push(async () => {
      throw new Error("host signaling failed");
    });
    const outcome = await useMultiplayerDraftStore.getState().hostDraft(hostConfig);
    expect(outcome).toBe("failed");
  });

  it("host: reports superseded when the session was left while it was pending", async () => {
    let failHost!: (e: Error) => void;
    hosts.nextInitialize.push(() => new Promise<void>((_resolve, reject) => {
      failHost = reject;
    }));
    const hosting = useMultiplayerDraftStore.getState().hostDraft(hostConfig);
    await vi.waitFor(() => expect(hosts.instances).toHaveLength(1));
    await useMultiplayerDraftStore.getState().leave(true);
    failHost(new Error("host signaling failed"));
    const outcome = await hosting;
    expect(outcome).toBe("superseded");
  });

  it("resumeDraft reports failed when its own reconnect fails", async () => {
    const locator = { roomCode: "OLD", displayName: "A", hostPeerId: "phase2-OLD", timestamp: 1 };
    persist.inspectActiveDraftGuest.mockReturnValue({ type: "present", meta: locator, capture: locator });
    persist.loadDraftGuestSession.mockResolvedValue({ draftToken: "tok" });
    guests.nextInitialize.push(async () => {
      throw new Error("handshake refused");
    });
    const outcome = await useMultiplayerDraftStore.getState().resumeDraft();
    expect(outcome).toBe("failed");
    expect(guests.instances).toHaveLength(1);
  });

  it("resumeDraft reports superseded when its reconnect is superseded", async () => {
    const locator = { roomCode: "OLD", displayName: "A", hostPeerId: "phase2-OLD", timestamp: 1 };
    persist.inspectActiveDraftGuest.mockReturnValue({ type: "present", meta: locator, capture: locator });
    persist.loadDraftGuestSession.mockResolvedValue({ draftToken: "tok" });
    const stale = gateNextJoin();
    const resuming = useMultiplayerDraftStore.getState().resumeDraft();
    await vi.waitFor(() => expect(net.joinRoom).toHaveBeenCalledTimes(1));
    await useMultiplayerDraftStore.getState().joinDraft({ kind: "new", roomCode: "NEWER", displayName: "B" });
    useConnectivityStore.setState({ forcedOffline: true });
    const before = useMultiplayerDraftStore.getState();
    const snapshot = { role: before.role, phase: before.phase, error: before.error };
    stale.release();
    const outcome = await resuming;
    const after = useMultiplayerDraftStore.getState();
    expect(outcome).toBe("superseded");
    expect({ role: after.role, phase: after.phase, error: after.error }).toEqual(snapshot);
  });
});
