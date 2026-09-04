import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Claim boundary: `audioDeviceSafe` is mocked here, so this file proves the
// DISABLE WIRING (verdict → disable → silent, completed boot) and its
// ordering. The service's own seam (isTauri, invoke, timeout race) is proven
// in services/__tests__/audioHealth.test.ts.
const { audioDeviceSafeMock } = vi.hoisted(() => ({
  audioDeviceSafeMock: vi.fn(),
}));
vi.mock("../../services/audioHealth", () => ({ audioDeviceSafe: audioDeviceSafeMock }));

// Swappable so a test can reproduce the WebKitGTK failure mode from issue
// #6744: a decode that neither resolves nor rejects because the pipeline it
// needs was never fully built.
let decodeAudioData: () => Promise<unknown> = () => Promise.resolve({});

const audioContextSpy = vi.fn().mockImplementation(function () {
  return {
    createGain: vi.fn().mockImplementation(() => ({
      gain: {
        value: 1,
        cancelScheduledValues: vi.fn(),
        setValueAtTime: vi.fn(),
        linearRampToValueAtTime: vi.fn(),
      },
      connect: vi.fn(),
    })),
    decodeAudioData: vi.fn().mockImplementation(() => decodeAudioData()),
    close: vi.fn(),
    destination: {},
    currentTime: 0,
  };
});
vi.stubGlobal("AudioContext", audioContextSpy);

vi.stubGlobal(
  "fetch",
  vi.fn().mockResolvedValue({ arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) }),
);

// Avoid IndexedDB in happy-dom.
vi.mock("../../audio/audioCache", () => ({
  fetchWithCache: vi.fn().mockResolvedValue(new ArrayBuffer(8)),
  getCachedManifest: vi.fn().mockResolvedValue(null),
  cacheThemeManifest: vi.fn().mockResolvedValue(undefined),
  clearThemeCache: vi.fn().mockResolvedValue(undefined),
}));

// preloadAssets caches its promise and audioManager latches isWarmedUp, both
// at module scope — fresh module graph per test (changelog.test.ts idiom).
async function freshBoot() {
  vi.resetModules();
  const preload = await import("../preloadAssets");
  const { audioManager } = await import("../../audio/AudioManager");
  const { useAudioHealthStore } = await import("../../stores/audioHealthStore");
  return { ...preload, audioManager, useAudioHealthStore };
}

describe("ensurePreload audio gate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    decodeAudioData = () => Promise.resolve({});
  });

  it("wedged verdict: boot completes silent, disable precedes gesture listeners", async () => {
    audioDeviceSafeMock.mockResolvedValue(false);
    const addListenerSpy = vi.spyOn(document, "addEventListener");
    const { ensurePreload, subscribePreload, audioManager, useAudioHealthStore } =
      await freshBoot();
    const disableSpy = vi.spyOn(audioManager, "disable");

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    await ensurePreload();
    unsub();

    // Silent: the device was never opened…
    expect(audioContextSpy).not.toHaveBeenCalled();
    // …but the boot still completed and the splash can dismiss.
    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBe("device-wedged");

    // Ordering, not mere co-occurrence: disable() must run before ANY gesture
    // listener exists, else a click during boot could still reach warmUp().
    expect(disableSpy).toHaveBeenCalledOnce();
    const disableOrder = disableSpy.mock.invocationCallOrder[0];
    const listenerOrders = addListenerSpy.mock.invocationCallOrder;
    expect(listenerOrders.length).toBeGreaterThan(0);
    for (const order of listenerOrders) {
      expect(disableOrder).toBeLessThan(order);
    }
    addListenerSpy.mockRestore();
  });

  // Control arm: flipping only the verdict flips the device-open observable,
  // so the wedged test's not-called assertion is discriminating.
  it("healthy verdict: warmUp opens the device and nothing is blocked", async () => {
    audioDeviceSafeMock.mockResolvedValue(true);
    const { ensurePreload, subscribePreload, useAudioHealthStore } = await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    await ensurePreload();
    unsub();

    expect(audioContextSpy).toHaveBeenCalledOnce();
    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBeNull();
  });

  it("progress starts moving before the verdict resolves (splash is never 0%-frozen)", async () => {
    let resolveVerdict!: (safe: boolean) => void;
    audioDeviceSafeMock.mockReturnValue(
      new Promise<boolean>((resolve) => {
        resolveVerdict = resolve;
      }),
    );
    const { ensurePreload, subscribePreload } = await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    const done = ensurePreload();

    expect(percents).toContain(10);
    expect(percents).not.toContain(100);

    resolveVerdict(true);
    await done;
    unsub();
    expect(percents).toContain(100);
  });
});

// Issue #6744. WebKitGTK builds every decode as a GStreamer pipeline; with no
// usable plugin set it logs the missing elements, wires a partial pipeline and
// leaves the promise pending forever. The device opened fine, so the wedged-
// device gate above cannot see this — only a deadline on the preload can.
// Every test here fails by TIMING OUT if the deadline is removed, which is
// exactly the user-visible bug: a splash that never dismisses.
describe("ensurePreload SFX deadline", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    audioDeviceSafeMock.mockResolvedValue(true);
    decodeAudioData = () => Promise.resolve({});
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("a decode that never settles still boots the app, degraded and diagnosed", async () => {
    decodeAudioData = () => new Promise(() => {});
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    const {
      ensurePreload,
      subscribePreload,
      audioManager,
      useAudioHealthStore,
      SFX_PRELOAD_DEADLINE_MS,
    } = await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    const done = ensurePreload();

    // The deadline is the only thing that can complete this boot.
    await vi.advanceTimersByTimeAsync(SFX_PRELOAD_DEADLINE_MS);
    await done;
    unsub();

    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBe("media-unavailable");
    // Latched off so no later pipeline (theme load, music, ensurePlayback)
    // opens another decode that will hang the same way.
    expect(audioManager.isDisabled).toBe(true);
    // Actionable, not just "audio failed": names the missing plugin set and
    // where to see which plugins (§ acceptance criterion 4).
    expect(diagnostic).toHaveBeenCalledOnce();
    expect(diagnostic.mock.calls[0][0]).toMatch(/GStreamer/);
    diagnostic.mockRestore();
  });

  // Control arm: same clock, same fake timers, decode resolves — so the test
  // above is discriminating about the hang and not about fake timers.
  it("a decode that settles boots with audio intact and no diagnostic", async () => {
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    const {
      ensurePreload,
      subscribePreload,
      audioManager,
      useAudioHealthStore,
      SFX_PRELOAD_DEADLINE_MS,
    } = await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    const done = ensurePreload();
    await vi.advanceTimersByTimeAsync(SFX_PRELOAD_DEADLINE_MS);
    await done;
    unsub();

    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBeNull();
    expect(audioManager.isDisabled).toBe(false);
    expect(diagnostic).not.toHaveBeenCalled();
    diagnostic.mockRestore();
  });

  // The decoder rejecting is the OTHER way a broken media stack shows up, and
  // it never reaches the deadline: `loadBuffer` catches per-file failures, so
  // `preloadSfx` fulfils promptly with nothing loaded. Driven through the real
  // preloadSfx → loadBuffer → decodeAudioData path — stubbing `preloadSfx`
  // itself would prove only that a rejected promise is caught, not that a
  // fulfilled-but-empty preload is recognised as failure.
  it("a decoder that rejects every file degrades boot without waiting for the deadline", async () => {
    decodeAudioData = () => Promise.reject(new Error("no decoder"));
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { ensurePreload, subscribePreload, audioManager, useAudioHealthStore } =
      await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    // No timer advance: every decode settles, so the deadline never fires.
    await expect(ensurePreload()).resolves.toBeUndefined();
    unsub();

    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBe("media-unavailable");
    expect(audioManager.isDisabled).toBe(true);
    expect(diagnostic).toHaveBeenCalledOnce();
    diagnostic.mockRestore();
    warn.mockRestore();
  });

  // Partial failure is not platform failure: one unreachable sound must leave
  // audio on. This is the arm that stops "report failure when a decode fails"
  // from being over-applied.
  it("one failed file among many keeps audio enabled", async () => {
    let call = 0;
    decodeAudioData = () => {
      call += 1;
      return call === 1 ? Promise.reject(new Error("one bad file")) : Promise.resolve({});
    };
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { ensurePreload, subscribePreload, audioManager, useAudioHealthStore } =
      await freshBoot();

    const percents: number[] = [];
    const unsub = subscribePreload((p) => percents.push(p.percent));
    await expect(ensurePreload()).resolves.toBeUndefined();
    unsub();

    // Reach guard: the failing file really was decoded, so this is a partial
    // failure and not a run where nothing was attempted.
    expect(call).toBeGreaterThan(1);
    expect(percents).toContain(100);
    expect(useAudioHealthStore.getState().unavailable).toBeNull();
    expect(audioManager.isDisabled).toBe(false);
    expect(diagnostic).not.toHaveBeenCalled();
    diagnostic.mockRestore();
    warn.mockRestore();
  });

  // The wedged-device verdict already named the fault and left ctx null, so
  // preloadSfx has nothing to do. It must not relabel that boot as a media
  // failure — "skipped" is not "none".
  it("a wedged device keeps its own reason and is not relabelled media-unavailable", async () => {
    audioDeviceSafeMock.mockResolvedValue(false);
    const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
    const { ensurePreload, useAudioHealthStore } = await freshBoot();

    await ensurePreload();

    expect(useAudioHealthStore.getState().unavailable).toBe("device-wedged");
    expect(diagnostic).not.toHaveBeenCalled();
    diagnostic.mockRestore();
  });
});
