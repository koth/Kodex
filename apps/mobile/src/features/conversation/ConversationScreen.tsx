import { useCallback, useEffect, useState } from "react";
import { View, Text, Pressable, ActivityIndicator, KeyboardAvoidingView, Platform } from "react-native";
import { useAppController, useSnapshot } from "../../app/AppServicesContext";
import { ConversationTimeline } from "./ConversationTimeline";
import { Composer } from "../composer/Composer";
import { PermissionApprovalSheet } from "../permission/PermissionApprovalSheet";
import { styles, colors, spacing } from "../theme";

interface Props {
  sessionId: string;
  title: string;
  onBack: () => void;
}

// Session view: header (title/status/cancel), the conversation timeline, the
// prompt composer, and an overlay permission approval sheet. The timeline is
// driven by the snapshot reducer so it stays byte-equivalent to the desktop.
export function ConversationScreen({ sessionId, title, onBack }: Props) {
  const controller = useAppController();
  const snapshot = useSnapshot();
  const [canceling, setCanceling] = useState(false);
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

  const handleStop = useCallback(
    (toolCallId: string) => controller.stopTool(toolCallId),
    [controller],
  );

  const handleCancel = async () => {
    setCanceling(true);
    try {
      await controller.cancel();
    } finally {
      setCanceling(false);
    }
  };

  useEffect(() => {
    let active = true;
    (async () => {
      try {
        await controller.switchSession(sessionId);
        await controller.getState(sessionId);
      } catch (e) {
        if (active) setSendError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      active = false;
    };
  }, [controller, sessionId]);

  const status = snapshot?.session.status ?? "Idle";
  const streaming = status === "Streaming" || status === "WaitingForTool";

  return (
    <KeyboardAvoidingView
      style={{ flex: 1 }}
      behavior={Platform.OS === "ios" ? "padding" : "height"}
      keyboardVerticalOffset={Platform.OS === "ios" ? 88 : 0}
      enabled={Platform.OS === "ios"}
    >
    <View style={styles.screen}>
      <View style={[styles.rowBetween, { padding: spacing.sm, borderBottomWidth: 1, borderBottomColor: colors.border }]}>
        <Pressable onPress={onBack} hitSlop={8}>
          <Text style={[styles.text, { color: colors.accent }]}>‹ Sessions</Text>
        </Pressable>
        <View style={{ flex: 1, marginHorizontal: spacing.sm }}>
          <Text style={[styles.text, { fontWeight: "600" }]} numberOfLines={1}>{title}</Text>
          <Text style={styles.status}>{status}</Text>
        </View>
        {canceling ? (
          <ActivityIndicator color={colors.accent} />
        ) : (
          <Pressable style={[styles.buttonGhost, { paddingVertical: spacing.xs, paddingHorizontal: spacing.sm }]} onPress={handleCancel} disabled={!streaming && status !== "Interrupted"}>
            <Text style={[styles.text, { fontSize: 13, color: colors.danger }]}>Cancel</Text>
          </Pressable>
        )}
      </View>

      {snapshot ? (
        <ConversationTimeline snapshot={snapshot} onStopTool={handleStop} />
      ) : (
        <View style={styles.center}>
          <ActivityIndicator color={colors.accent} />
          <Text style={[styles.textDim, { marginTop: spacing.sm }]}>Syncing session…</Text>
        </View>
      )}

      <Composer onSend={handleSend} disabled={!snapshot} error={sendError} />

      <PermissionApprovalSheet />
    </View>
    </KeyboardAvoidingView>
  );
}
// end of file
