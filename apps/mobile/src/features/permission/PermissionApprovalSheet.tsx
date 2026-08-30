import { useState } from "react";
import { View, Text, TextInput, Pressable, Modal, ScrollView } from "react-native";
import { useAppController, useSnapshot, usePendingApprovals } from "../../app/AppServicesContext";
import { isDestructive } from "../../session/permission";
import type { PermissionInputResponse } from "../../types";
import { styles, colors, spacing, radius, shadows } from "../theme";

// Default-deny permission approval. The phone is the SOLE approval gate for
// destructive remote operations: no "allow" is preselected, and destructive
// ops require an explicit second confirmation before ResolvePermission is
// sent. Non-destructive (read-only) ops can be approved in one step.
export function PermissionApprovalSheet() {
  const controller = useAppController();
  const snapshot = useSnapshot();
  const pending = usePendingApprovals();
  const [confirming, setConfirming] = useState<string | null>(null);
  const [textInputs, setTextInputs] = useState<Record<string, string>>({});

  const toolById = new Map((snapshot?.tools ?? []).map((tool) => [tool.call_id, tool]));
  const approval = pending[0];

  if (!approval) return null;
  const tool = approval.toolCallId ? toolById.get(approval.toolCallId) : undefined;
  const destructive = tool ? isDestructive(tool) : true;
  const options = tool?.permission_options ?? [];
  const inputQuestions = (approval.request?.questions ?? []).filter((q) => q.is_secret || q.is_other);

  const resolve = async (optionId: string | null) => {
    const answers: Record<string, string[]> = {};
    for (const question of inputQuestions) {
      const value = textInputs[question.id]?.trim();
      if (value) answers[question.id] = [value];
    }
    const inputResponse: PermissionInputResponse | null =
      Object.keys(answers).length > 0 ? { answers } : null;
    if (inputResponse) {
      await controller.approvePermissionWithInput(approval.permissionRequestId, optionId, null, inputResponse);
    } else {
      await controller.approvePermission(approval.permissionRequestId, optionId);
    }
    setConfirming(null);
    setTextInputs({});
  };

  const requireConfirm = destructive && confirming !== approval.permissionRequestId;
  const allowOption = options.find((option) => /allow|yes|approve|once/i.test(option.label));

  return (
    <Modal visible transparent animationType="slide" onRequestClose={() => controller.denyPermission(approval.permissionRequestId)}>
      <View style={{ flex: 1, justifyContent: "flex-end", backgroundColor: colors.scrim }}>
        <View style={{ backgroundColor: colors.surface, borderTopLeftRadius: radius.xl, borderTopRightRadius: radius.xl, maxHeight: "85%", borderWidth: 1, borderColor: colors.border, ...shadows.raised }}>
          <View style={{ alignSelf: "center", width: 40, height: 5, borderRadius: 3, backgroundColor: colors.borderStrong, marginTop: spacing.sm }} />
          <ScrollView contentContainerStyle={{ padding: spacing.lg, paddingTop: spacing.md }}>
            <View style={styles.rowBetween}>
              <Text style={[styles.text, { fontWeight: "800", fontSize: 17 }]}>请求授权</Text>
              {destructive ? (
                <View style={[styles.chip, { backgroundColor: colors.dangerTint, borderColor: colors.danger }]}>
                  <Text style={{ color: colors.danger, fontSize: 10, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.5 }}>Destructive</Text>
                </View>
              ) : null}
            </View>

            {tool ? (
              <>
                <View style={[styles.row, { marginTop: spacing.md }]}>
                  <Text style={[styles.mono, { color: colors.accent, fontSize: 12 }]}>{tool.name}</Text>
                </View>
                {tool.summary ? <Text style={[styles.text, { marginTop: spacing.xs, fontSize: 14, lineHeight: 20 }]}>{tool.summary}</Text> : null}
                {tool.detail_text ? <Text style={[styles.textDim, { marginTop: spacing.xs }]}>{tool.detail_text}</Text> : null}
              </>
            ) : (
              <Text style={[styles.textDim, { marginTop: spacing.sm }]}>{approval.toolName}</Text>
            )}

            {inputQuestions.length > 0 ? (
              <View style={{ marginTop: spacing.md }}>
                {inputQuestions.map((question) => (
                  <View key={question.id} style={{ marginTop: spacing.sm }}>
                    <Text style={[styles.text, { fontSize: 13, fontWeight: "600" }]}>{question.question}</Text>
                    <TextInput
                      style={[styles.input, { marginTop: spacing.xs }]}
                      placeholder={question.is_secret ? "secret input" : "free text"}
                      placeholderTextColor={colors.textFaint}
                      secureTextEntry={question.is_secret}
                      value={textInputs[question.id] ?? ""}
                      onChangeText={(value) => setTextInputs((prev) => ({ ...prev, [question.id]: value }))}
                    />
                  </View>
                ))}
              </View>
            ) : null}

            {requireConfirm ? (
              <View style={{ marginTop: spacing.lg }}>
                <View style={[styles.chip, { backgroundColor: colors.dangerTint, borderColor: colors.danger, alignSelf: "flex-start" }]}>
                  <Text style={{ color: colors.danger, fontSize: 12 }}>此操作可能修改你的工作区,确认后放行。</Text>
                </View>
                <View style={[styles.row, { marginTop: spacing.md }]}>
                  <Pressable
                    style={({ pressed }) => [styles.buttonDanger, { flex: 1, marginRight: spacing.sm, opacity: pressed ? 0.9 : 1 }]}
                    onPress={() => resolve(allowOption?.id ?? null)}
                  >
                    <Text style={styles.buttonText}>确认放行</Text>
                  </Pressable>
                  <Pressable
                    style={({ pressed }) => [styles.buttonGhost, { flex: 1, opacity: pressed ? 0.7 : 1 }]}
                    onPress={() => controller.denyPermission(approval.permissionRequestId)}
                  >
                    <Text style={[styles.text, { fontWeight: "600" }]}>拒绝</Text>
                  </Pressable>
                </View>
              </View>
            ) : (
              <View style={[styles.row, { flexWrap: "wrap", marginTop: spacing.lg }]}>
                {options.map((option) => {
                  const isAllow = /allow|yes|approve|once/i.test(option.label);
                  const isDeny = /deny|no|cancel|block/i.test(option.label);
                  return (
                    <Pressable
                      key={option.id}
                      style={({ pressed }) => [
                        isDeny ? styles.buttonDanger : styles.buttonGhost,
                        { marginRight: spacing.xs, marginBottom: spacing.xs, paddingVertical: spacing.sm + 1, opacity: pressed ? 0.85 : 1 },
                      ]}
                      onPress={() => (destructive && isAllow ? setConfirming(approval.permissionRequestId) : resolve(option.id))}
                    >
                      <Text style={[styles.text, { fontWeight: "600" }, isDeny && { color: "#fff" }]}>{option.label}</Text>
                    </Pressable>
                  );
                })}
                <Pressable
                  style={({ pressed }) => [styles.buttonGhost, { paddingVertical: spacing.sm + 1, opacity: pressed ? 0.7 : 1 }]}
                  onPress={() => controller.denyPermission(approval.permissionRequestId)}
                >
                  <Text style={[styles.text, { color: colors.danger, fontWeight: "600" }]}>拒绝</Text>
                </Pressable>
              </View>
            )}
          </ScrollView>
        </View>
      </View>
    </Modal>
  );
}
// end of file