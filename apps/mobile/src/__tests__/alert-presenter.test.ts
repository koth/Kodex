import { describe, it, expect } from "vitest";
import {
  CompositeAlertPresenter,
  type AlertEnvironment,
  type AlertPorts,
} from "../features/notifications/presenter";
import {
  DEFAULT_ALERT_SETTINGS,
  type AlertSettings,
} from "../features/notifications/settings";
import type { TurnCompletionContext } from "../session/turn-completion";

// Spec: mobile-turn-completion-alerts — Context-aware alert presentation,
// Background-only mode, Alert settings and persistence.

const CTX: TurnCompletionContext = {
  sessionId: "s-1",
  sessionTitle: "Demo",
  turnStartedAtMs: 1000,
};

function makeHarness(
  env: Partial<AlertEnvironment>,
  settings: Partial<AlertSettings> = {},
) {
  const calls: string[] = [];
  const ports: AlertPorts = {
    haptics: {
      subtle: () => calls.push("haptic:subtle"),
      success: () => calls.push("haptic:success"),
      warning: () => calls.push("haptic:warning"),
    },
    sound: {
      playCompletion: () => calls.push("sound:complete"),
      playInterruption: () => calls.push("sound:interrupted"),
    },
    banner: { show: (_ctx, outcome) => calls.push(`banner:${outcome}`) },
    notify: {
      notify: (_ctx, outcome, opts) =>
        calls.push(`notify:${outcome}:sound=${opts.sound}:vibration=${opts.vibration}`),
    },
  };
  const environment: AlertEnvironment = {
    appState: () => "active",
    isViewingSession: () => false,
    ...env,
  };
  const merged: AlertSettings = { ...DEFAULT_ALERT_SETTINGS, ...settings };
  const presenter = new CompositeAlertPresenter(ports, environment, () => merged);
  return { calls, presenter };
}

describe("CompositeAlertPresenter", () => {
  it("downgrades to a subtle haptic when the user is watching that conversation", () => {
    const h = makeHarness({ isViewingSession: (id) => id === "s-1" });
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual(["haptic:subtle"]);
  });

  it("gives the full foreground alert on any other screen", () => {
    const h = makeHarness({});
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual(["sound:complete", "haptic:success", "banner:completed"]);
  });

  it("uses warning feedback for interruptions", () => {
    const h = makeHarness({});
    h.presenter.onTurnInterrupted(CTX);
    expect(h.calls).toEqual(["sound:interrupted", "haptic:warning", "banner:interrupted"]);
  });

  it("posts a system notification when backgrounded", () => {
    const h = makeHarness({ appState: () => "background" });
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual(["notify:completed:sound=true:vibration=true"]);
  });

  it("honours the sound/vibration toggles in the backgrounded notification", () => {
    const h = makeHarness({ appState: () => "inactive" }, { sound: false });
    h.presenter.onTurnInterrupted(CTX);
    expect(h.calls).toEqual(["notify:interrupted:sound=false:vibration=true"]);
  });

  it("stays silent in the foreground when background-only mode is on", () => {
    const h = makeHarness({}, { backgroundOnly: true });
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual([]);
  });

  it("still notifies in the background when background-only mode is on", () => {
    const h = makeHarness({ appState: () => "background" }, { backgroundOnly: true });
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual(["notify:completed:sound=true:vibration=true"]);
  });

  it("does nothing when the master switch is off", () => {
    for (const appState of ["active", "background"]) {
      const h = makeHarness({ appState: () => appState }, { enabled: false });
      h.presenter.onTurnCompleted(CTX);
      h.presenter.onTurnInterrupted(CTX);
      expect(h.calls).toEqual([]);
    }
  });

  it("respects the sound toggle in the foreground", () => {
    const h = makeHarness({}, { sound: false });
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual(["haptic:success", "banner:completed"]);
  });

  it("respects the vibration toggle when watching the conversation", () => {
    const h = makeHarness(
      { isViewingSession: () => true },
      { vibration: false },
    );
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual([]);
  });

  it("does not notify in the background when system notifications are off", () => {
    const h = makeHarness(
      { appState: () => "background" },
      { systemNotifications: false },
    );
    h.presenter.onTurnCompleted(CTX);
    expect(h.calls).toEqual([]);
  });
});
