import { createNavigationContainerRef } from "@react-navigation/native";
import type { RootStackParamList } from "./navigation";

// Navigation handle for non-React code (alert banner taps, the route probe
// used by the turn-completion presenter). Type-only import keeps the cycle
// navigation.tsx <-> navigation-ref.ts erased at runtime.
export const navigationRef = createNavigationContainerRef<RootStackParamList>();

/** True when the visible route is the conversation screen of `sessionId`. */
export function isViewingConversation(sessionId: string): boolean {
  if (!navigationRef.isReady()) return false;
  const route = navigationRef.getCurrentRoute();
  return (
    route?.name === "Conversation" &&
    (route.params as { sessionId?: string } | undefined)?.sessionId === sessionId
  );
}
