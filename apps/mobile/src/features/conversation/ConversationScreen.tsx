import { useCallback, useEffect, useState } from "react";
import { View, Text, Pressable, ActivityIndicator, KeyboardAvoidingView, Platform } from "react-native";
import { useAppController, useSnapshot } from "../../app/AppServicesContext";
import { ConversationTimeline } from "./ConversationTimeline";
import { Composer } from "../composer/Composer";
import { PermissionApprovalSheet } from "../permission/PermissionApprovalSheet";
import { styles, colors, spacing, radius } from "../theme";

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

  const status = snapshot?.session.status ?? "Idle";
  const streaming = status === "Streaming" || status === "WaitingForTool";
  const statusTint =
    status === "Streaming" || status === "WaitingForTool"
      ? { color: colors.success, bg: colors.successTint, border: colors.success }
      : status === "Interrupted"
        ? { color: colors.danger, bg: colors.dangerTint, border: colors.danger }
        : { color: colors.textDim, bg: colors.surfaceAlt, border: colors.border };

  return (
    <KeyboardAvoidingView
      style={{ flex: 1 }}
      behavior={Platform.OS === "ios" ? "padding" : "height"}
      keyboardVerticalOffset={Platform.OS === "ios" ? 88 : 0}
      enabled={Platform.OS === "ios"}
    >
      <View style={styles.screen}>
        <View style={[styles.rowBetween, { padding: spacing.md, borderBottomWidth: 1, borderBottomColor: colors.border }]}>
          <Pressable
            onPress={onBack}
            hitSlop={10}
            style={({ pressed }) => ({
              flexDirection: "row",
              alignItems: "center",
              paddingVertical: spacing.xs,
              paddingHorizontal: spacing.sm,
              borderRadius: radius.pill,
              opacity: pressed ? 0.7 : 1,
            })}
          >
            <Text style={{ color: colors.accent, fontSize: 22, fontWeight: "300", marginRight: 2 }}>{"\u2039"}</Text>
            <Text style={{ color: colors.accent, fontSize: 15, fontWeight: "600" }}>Sessions</Text>
          </Pressable>
          <View style={{ flex: 1, marginHorizontal: spacing.sm, minWidth: 0 }}>
            <Text style={[styles.text, { fontWeight: "700", fontSize: 15 }]} numberOfLines={1}>{title}</Text>
            <View style={[styles.row, { marginTop: 2 }]}>
              <View style={[styles.chip, { backgroundColor: statusTint.bg, borderColor: statusTint.border, paddingVertical: 2 }]}>
                <Text style={{ color: statusTint.color, fontSize: 10, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.4 }}>
                  {streaming ? "live" : status.toLowerCase()}
                </Text>
              </View>
            </View>
          </View>
          {canceling ? (
            <ActivityIndicator color={colors.accent} />
          ) : (
            <Pressable
              style={({ pressed }) => [styles.buttonGhost, { paddingVertical: spacing.xs + 1, paddingHorizontal: spacing.md, opacity: pressed ? 0.7 : 1 }]}
              onPress={handleCancel}
              disabled={!streaming && status !== "Interrupted"}
            >
              <Text style={[styles.text, { fontSize: 13, fontWeight: "600", color: colors.danger }]}>Cancel</Text>
            </Pressable>
          )}
        </View>

        {snapshot ? (
          <ConversationTimeline snapshot={snapshot} onStopTool={handleStop} />
        ) : sendError ? (
          <View style={styles.center}>
            <Text style={[styles.text, { color: colors.danger, textAlign: "center" }]}>
              {sendError}
            </Text>
          </View>
        ) : (
          <View style={styles.center}>
            <ActivityIndicator color={colors.accent} />
            <Text style={[styles.textDim, { marginTop: spacing.sm }]}>{"Syncing session\u2026"}</Text>
          </View>
        )}

        <Composer onSend={handleSend} disabled={!snapshot} error={sendError} />

        <PermissionApprovalSheet />
      </View>
    </KeyboardAvoidingView>
  );
}
// end of file