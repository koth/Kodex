import type {
  AlertPresenter,
  TurnCompletionContext,
  TurnOutcome,
} from "../../session/turn-completion";
import { diagnostics } from "../../util/diagnostics";
import type { AlertSettings } from "./settings";

// Alert presentation policy (spec: mobile-turn-completion-alerts —
// Context-aware alert presentation / Background-only mode / Alert settings).
//
// The presenter is pure orchestration over injected ports: the native
// adapters (expo-haptics / expo-audio / expo-notifications / React banner)
// live in sibling modules and are only wired in the app shell, so this file
// — and its tests — never import native code.
//
// Policy table (design D3):
// | context                                   | sound | haptic | banner | system notification |
// | app active AND viewing that conversation  |   —   | light  |   —    |         —           |
// | app active, any other screen              |   ✓   |   ✓    |   ✓    |         —           |
// | app backgrounded                          |   —   |   —    |   —    | ✓ (sound+vibration) |
//
// `backgroundOnly` silences the two foreground rows; the master switch
// silences everything.

export interface HapticsPort {
  /** Subtle tap for the "user is already watching" case (medium impact —
   * a light impact proved imperceptible on several devices). */
  subtle(): void;
  /** Turn completed. */
  success(): void;
  /** Turn interrupted. */
  warning(): void;
}

export interface SoundPort {
  playCompletion(): void;
  playInterruption(): void;
}

export interface BannerPort {
  show(ctx: TurnCompletionContext, outcome: TurnOutcome): void;
}

export interface NotifyPort {
  notify(
    ctx: TurnCompletionContext,
    outcome: TurnOutcome,
    opts: { sound: boolean; vibration: boolean },
  ): void;
}

export interface AlertEnvironment {
  /** React Native AppState: "active" | "background" | "inactive" | … */
  appState(): string;
  /** True when the conversation screen for this session is the visible route. */
  isViewingSession(sessionId: string): boolean;
}

export interface AlertPorts {
  haptics: HapticsPort;
  sound: SoundPort;
  banner: BannerPort;
  notify: NotifyPort;
}

export class CompositeAlertPresenter implements AlertPresenter {
  constructor(
    private readonly ports: AlertPorts,
    private readonly env: AlertEnvironment,
    private readonly settings: () => AlertSettings,
  ) {}

  onTurnCompleted(ctx: TurnCompletionContext): void {
    this.present(ctx, "completed");
  }

  onTurnInterrupted(ctx: TurnCompletionContext): void {
    this.present(ctx, "interrupted");
  }

  private present(ctx: TurnCompletionContext, outcome: TurnOutcome): void {
    const s = this.settings();
    if (!s.enabled) {
      diagnostics.log("alerts", `${outcome} suppressed: master switch off`);
      return;
    }

    if (this.env.appState() === "active") {
      if (s.backgroundOnly) {
        diagnostics.log("alerts", `${outcome} suppressed: background-only mode`);
        return;
      }
      if (this.env.isViewingSession(ctx.sessionId)) {
        // The user is watching the conversation: the result renders in place,
        // so anything louder than a subtle tap is noise.
        diagnostics.log("alerts", `${outcome} foreground/viewing → subtle haptic only`);
        if (s.vibration) this.ports.haptics.subtle();
        return;
      }
      diagnostics.log(
        "alerts",
        `${outcome} foreground/elsewhere → sound=${s.sound} vibration=${s.vibration} banner`,
      );
      if (s.sound) {
        if (outcome === "completed") this.ports.sound.playCompletion();
        else this.ports.sound.playInterruption();
      }
      if (s.vibration) {
        if (outcome === "completed") this.ports.haptics.success();
        else this.ports.haptics.warning();
      }
      this.ports.banner.show(ctx, outcome);
      return;
    }

    diagnostics.log(
      "alerts",
      `${outcome} backgrounded → system notification (sound=${s.sound} vibration=${s.vibration} enabled=${s.systemNotifications})`,
    );

    // Backgrounded (or transitioning): the system notification carries sound
    // + vibration per the user's toggles. When the OS denies the permission
    // the adapter no-ops (degrade-to-foreground-only per spec).
    if (s.systemNotifications) {
      this.ports.notify.notify(ctx, outcome, { sound: s.sound, vibration: s.vibration });
    }
  }
}
