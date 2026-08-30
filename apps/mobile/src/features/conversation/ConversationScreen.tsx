import { useCallback, useEffect, useState } from "react";
import { View, Text, ActivityIndicator, KeyboardAvoidingView, Platform } from "react-native";
import { useAppController, useSnapshot } from "../../app/AppServicesContext";
import { ConversationTimeline } from "./ConversationTimeline";
import { TurnChangesBar } from "./TurnChangesBar";
import { SessionInfoSheet } from "./SessionInfoSheet";
import { Composer } from "../composer/Composer";
import { PermissionApprovalSheet } from "../permission/PermissionApprovalSheet";
import { styles, colors, spacing } from "../theme";

interface Props {
  sessionId: string;
}

// Session view: the conversation timeline, the prompt composer (which carries
// the stop button while a turn is running), and an overlay permission sheet.
// Chrome (back navigation, session title) belongs to the native stack header —
// a second in-screen header row only repeated it. The timeline is driven by
// the snapshot reducer so it stays byte-equivalent to the desktop.
export function ConversationScreen({ sessionId }: Props) {
  const controller = useAppController();
  const snapshot = useSnapshot();
  const [sendError, setSendError] = useState<string | null>(null);

  const handleSend = useCallback(
    async (text: string) => {
      setSendError(null);
      try {
        await controller.sendPrompt(text);
      } catch (e) {
        setSendError(e instanceof Error ? e.message : String(e));
        throw e;
      }
    },
    [controller],
  );

  const handleCancel = useCallback(async () => {
    await controller.cancel();
  }, [controller]);

  const handleStopTool = useCallback(
    (toolCallId: string) => controller.stopTool(toolCallId),
    [controller],
  );

  useEffect(() => {
    let active = true;
    let fallback: ReturnType<typeof setTimeout> | null = null;
    // Entry sync: the PC pushes a Full snapshot over the event channel as
    // soon as it processes the SwitchSession (its UiUpdated broadcast wakes
    // the relay event source). The switch request itself is always sent —
    // it is idempotent when the session is already active and repairs the
    // active session when the desktop user switched away locally. Only when
    // the store was actually wiped (cross-session entry) AND the push has
    // not landed within a short window — older PC build or a dead event
    // stream — do we pay for an explicit (duplicate) full GetState.
    (async () => {
      try {
        await controller.switchSession(sessionId);
        fallback = setTimeout(() => {
          if (!active || controller.snapshot) return;
          void controller
            .getState(sessionId)
            .catch((e: unknown) => {
              if (active) setSendError(e instanceof Error ? e.message : String(e));
            });
        }, 1500);
      } catch (e) {
        if (active) setSendError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      active = false;
      if (fallback !== null) clearTimeout(fallback);
    };
  }, [controller, sessionId]);

  const streaming =
    snapshot?.session.status === "Streaming" || snapshot?.session.status === "WaitingForTool";

  return (
    <KeyboardAvoidingView
      style={{ flex: 1 }}
      behavior={Platform.OS === "ios" ? "padding" : "height"}
      keyboardVerticalOffset={Platform.OS === "ios" ? 88 : 0}
      enabled={Platform.OS === "ios"}
    >
      <View style={styles.screen}>
        {snapshot ? (
          <ConversationTimeline snapshot={snapshot} onStopTool={handleStopTool} />
        ) : sendError ? (
          <View style={styles.center}>
            <Text style={[styles.text, { color: colors.danger, textAlign: "center" }]}>
              {sendError}
            </Text>
          </View>
        ) : (
          <View style={styles.center}>
            <ActivityIndicator color={colors.accent} />
            <Text style={[styles.textDim, { marginTop: spacing.sm }]}>{"正在同步会话\u2026"}</Text>
          </View>
        )}

        {snapshot ? <TurnChangesBar snapshot={snapshot} /> : null}

        <Composer
          onSend={handleSend}
          disabled={!snapshot}
          error={sendError}
          streaming={streaming}
          onCancel={handleCancel}
        />

        <SessionInfoSheet />
        <PermissionApprovalSheet />
      </View>
    </KeyboardAvoidingView>
  );
}
// end of file
