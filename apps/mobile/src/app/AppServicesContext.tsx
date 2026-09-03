import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { AppState } from "react-native";
import { AppController } from "./services";
import { SecureSecretStore } from "./secure-store";
import { FileLogSink } from "./diagnostics-sink";
import { diagnostics } from "../util/diagnostics";
import { WsTransport } from "../relay/transport";
import { CompositeAlertPresenter } from "../features/notifications/presenter";
import { expoHaptics } from "../features/notifications/haptics";
import { expoSound } from "../features/notifications/sound";
import { bannerPort } from "../features/notifications/banner";
import {
  ensureNotificationSetup,
  localNotifier,
  requestNotificationPermission,
} from "../features/notifications/local-notification";
import { isViewingConversation } from "./navigation-ref";
import type { PendingApproval } from "../session/permission";
import type { ConnectionState } from "../relay/state-machine";
import type { SubscriptionState } from "../account/subscription";
import type { AlertSettings } from "../features/notifications/settings";
import type { UiSnapshot } from "../types";

// React glue for the AppController. Constructs a single controller backed by
// the Keychain/Keystore SecretStore; screens consume it via the hooks below.
// The controller is framework-agnostic so it stays unit-testable.

const AppServicesContext = createContext<AppController | null>(null);

export function AppServicesProvider({ children }: { children: ReactNode }) {
  const [controller] = useState<AppController>(
    () => {
      diagnostics.setSink(new FileLogSink());
      return new AppController(
        new SecureSecretStore(),
        async (endpoint) => {
          const transport = new WsTransport(endpoint);
          await transport.ready;
          return transport;
        },
      );
    },
  );

  useEffect(() => {
    void (async () => {
      await controller.boot();
      // Request POST_NOTIFICATIONS proactively at launch (Android 13+).
      // The settings toggle's on-change request never fires when the toggle
      // already sits at its default ON value, so waiting for the user to
      // toggle it means notifications silently never work.
      if (controller.alertSettings.enabled && controller.alertSettings.systemNotifications) {
        const granted = await requestNotificationPermission();
        diagnostics.log("alerts", `boot permission request granted=${granted}`);
      }
    })();
    return () => {
      void controller.disconnect();
    };
  }, [controller]);

  // Turn-completion alerts: wire the native adapters into the controller's
  // watcher. The environment probes are read lazily at alert time so the
  // policy always sees the current app/route state.
  useEffect(() => {
    void ensureNotificationSetup();
    controller.setAlertPresenter(
      new CompositeAlertPresenter(
        {
          haptics: expoHaptics,
          sound: expoSound,
          banner: bannerPort,
          notify: localNotifier,
        },
        {
          appState: () => AppState.currentState,
          isViewingSession: (sessionId) => isViewingConversation(sessionId),
        },
        () => controller.alertSettings,
      ),
    );
    return () => controller.setAlertPresenter(null);
  }, [controller]);

  return (
    <AppServicesContext.Provider value={controller}>
      {children}
    </AppServicesContext.Provider>
  );
}

export function useAppController(): AppController {
  const controller = useContext(AppServicesContext);
  if (!controller) {
    throw new Error("useAppController must be used within <AppServicesProvider>");
  }
  return controller;
}

export function useConnectionState(): ConnectionState {
  const controller = useAppController();
  const [state, setState] = useState<ConnectionState>(controller.connectionState);
  useEffect(
    () => controller.connState.subscribe(setState),
    [controller],
  );
  return state;
}

export function useSnapshot(): UiSnapshot | null {
  const controller = useAppController();
  const [snapshot, setSnapshot] = useState<UiSnapshot | null>(controller.snapshot);
  useEffect(
    () => controller.sessionStore.subscribe(setSnapshot),
    [controller],
  );
  return snapshot;
}

export function usePendingApprovals(): PendingApproval[] {
  const controller = useAppController();
  const [pending, setPending] = useState<PendingApproval[]>(
    controller.pendingApprovals,
  );
  useEffect(() => controller.permissions.subscribe(setPending), [controller]);
  return pending;
}

export function useSubscriptionState(): SubscriptionState {
  const controller = useAppController();
  const [state, setState] = useState<SubscriptionState>(controller.subscriptionState);
  useEffect(() => {
    controller.setSubscriptionListener(setState);
    return () => controller.setSubscriptionListener(() => {});
  }, [controller]);
  return state;
}

export function useAlertSettings(): AlertSettings {
  const controller = useAppController();
  const [settings, setSettings] = useState<AlertSettings>(controller.alertSettings);
  useEffect(() => controller.subscribeAlertSettings(setSettings), [controller]);
  return settings;
}
// end of file
