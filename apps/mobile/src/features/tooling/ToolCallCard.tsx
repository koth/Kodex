import { memo, useState } from "react";
import { View, Text, Pressable } from "react-native";
import type { ToolInvocation } from "../../types";
import { styles, colors, spacing, radius, shadows } from "../theme";

interface Props {
  tool: ToolInvocation;
  onStop?: (toolCallId: string) => void;
}

type Tab = "summary" | "diff" | "logs" | "raw";

function tabLabel(tool: ToolInvocation, tab: Tab): string {
  if (tab === "summary") return "Summary";
  if (tab === "diff") return `Diff${tool.diff_previews.length ? ` (${tool.diff_previews.length})` : ""}`;
  if (tab === "logs") return `Logs${tool.logs.length ? ` (${tool.logs.length})` : ""}`;
  return "Raw";
}

function tabDisabled(tool: ToolInvocation, tab: Tab): boolean {
  if (tab === "diff") return tool.diff_previews.length === 0;
  if (tab === "logs") return tool.logs.length === 0;
  if (tab === "raw") return !tool.raw_input && !tool.raw_output;
  return false;
}

function statusTint(status: string): { color: string; bg: string; border: string } {
  if (status === "Succeeded") return { color: colors.success, bg: colors.successTint, border: colors.success };
  if (status === "Failed" || status === "Interrupted")
    return { color: colors.danger, bg: colors.dangerTint, border: colors.danger };
  if (status === "Pending" || status === "Running")
    return { color: colors.warn, bg: colors.warnTint, border: colors.warn };
  return { color: colors.textDim, bg: colors.surfaceAlt, border: colors.border };
}

// Fold/expand tool invocation card: summary + status, expandable diff
// previews, logs, and raw input/output. Ported from the desktop ToolCallCard
// but rendered with RN primitives (no Monaco/diff lib in the mobile MVP).
function ToolCallCardImpl({ tool, onStop }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [section, setSection] = useState<Tab>("summary");
  const running = tool.status === "Pending" || tool.status === "Running";
  const tint = statusTint(tool.status);

  const tabs: Tab[] = ["summary", "diff", "logs", "raw"];

  return (
    <View style={[styles.card, { padding: spacing.md, marginHorizontal: 0, marginVertical: spacing.xs, ...shadows.card }]}>
      <Pressable
        style={({ pressed }) => [styles.rowBetween, { opacity: pressed ? 0.7 : 1 }]}
        onPress={() => setExpanded((e) => !e)}
      >
        <View style={[styles.row, { flex: 1, flexWrap: "wrap" }]}>
          <Text style={{ color: colors.text, fontFamily: "monospace", fontSize: 12, fontWeight: "700" }}>{tool.name}</Text>
          {tool.kind && tool.kind !== "permission" ? (
            <View style={[styles.chip, { marginLeft: spacing.xs, backgroundColor: colors.surfaceAlt, borderColor: colors.border }]}>
              <Text style={{ color: colors.textDim, fontSize: 10, fontWeight: "600", textTransform: "uppercase", letterSpacing: 0.4 }}>{tool.kind}</Text>
            </View>
          ) : null}
        </View>
        <View style={[styles.chip, { backgroundColor: tint.bg, borderColor: tint.border }]}>
          <Text style={{ color: tint.color, fontSize: 10, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.4 }}>{tool.status}</Text>
        </View>
      </Pressable>

      {tool.summary ? (
        <Pressable onPress={() => setExpanded((e) => !e)}>
          <Text style={[styles.text, { marginTop: spacing.xs, fontSize: 14, lineHeight: 20 }]}>{tool.summary}</Text>
        </Pressable>
      ) : null}

      {expanded ? (
        <View style={{ marginTop: spacing.md }}>
          <View style={[styles.row, { marginBottom: spacing.sm, flexWrap: "wrap" }]}>
            {tabs.map((tab) => {
              const active = section === tab;
              const disabled = tabDisabled(tool, tab);
              return (
                <Pressable
                  key={tab}
                  onPress={() => setSection(tab)}
                  disabled={disabled}
                  style={[
                    styles.chip,
                    { marginRight: spacing.xs, marginBottom: spacing.xs, backgroundColor: active ? colors.accent : colors.surfaceAlt, borderColor: active ? colors.accent : colors.border },
                    disabled && { opacity: 0.35 },
                  ]}
                >
                  <Text style={{ color: active ? "#fff" : colors.textDim, fontSize: 11, fontWeight: "600" }}>{tabLabel(tool, tab)}</Text>
                </Pressable>
              );
            })}
          </View>

          {section === "summary" && tool.detail_text ? (
            <Text style={styles.textDim}>{tool.detail_text}</Text>
          ) : null}
          {section === "summary" && tool.error ? (
            <View style={[styles.chip, { marginTop: spacing.xs, backgroundColor: colors.dangerTint, borderColor: colors.danger, alignSelf: "flex-start" }]}>
              <Text style={{ color: colors.danger, fontSize: 12 }}>{tool.error}</Text>
            </View>
          ) : null}

          {section === "diff"
            ? tool.diff_previews.map((preview) => (
                <View key={preview.path} style={{ marginTop: spacing.xs }}>
                  <View style={[styles.row, { backgroundColor: colors.surfaceAlt, borderRadius: radius.sm, paddingHorizontal: spacing.sm, paddingVertical: 4 }]}>
                    <Text style={[styles.mono, { color: colors.textDim, fontSize: 12 }]}>{preview.path}</Text>
                  </View>
                  {preview.hunks.map((hunk, hi) => (
                    <View key={`${preview.path}:${hi}`} style={{ marginTop: spacing.xs }}>
                      {hunk.heading ? (
                        <Text style={[styles.mono, { color: colors.textFaint, fontSize: 11 }]}>{hunk.heading}</Text>
                      ) : null}
                      {hunk.lines.map((line, li) => (
                        <Text
                          key={`${preview.path}:${hi}:${li}`}
                          style={[
                            styles.mono,
                            {
                              color: line.kind === "Added" ? colors.success : line.kind === "Removed" ? colors.danger : colors.textDim,
                              fontSize: 12,
                              lineHeight: 18,
                            },
                          ]}
                        >
                          {line.kind === "Added" ? "+ " : line.kind === "Removed" ? "- " : "  "}
                          {line.content}
                        </Text>
                      ))}
                    </View>
                  ))}
                </View>
              ))
            : null}

          {section === "logs"
            ? tool.logs.map((entry, idx) => (
                <View key={`log:${idx}`} style={{ marginTop: spacing.xs, backgroundColor: colors.surfaceAlt, borderRadius: radius.sm, padding: spacing.sm }}>
                  <Text style={[styles.text, { fontWeight: "700", fontSize: 12 }]}>{entry.title}</Text>
                  {entry.body ? <Text style={[styles.mono, { marginTop: 4 }]}>{entry.body}</Text> : null}
                </View>
              ))
            : null}

          {section === "raw" ? (
            <View>
              {tool.raw_input ? (
                <View style={{ marginTop: spacing.xs }}>
                  <Text style={[styles.textFaint, { fontSize: 11, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.5 }]}>input</Text>
                  <Text style={[styles.mono, { marginTop: 2 }]}>{tool.raw_input}</Text>
                </View>
              ) : null}
              {tool.raw_output ? (
                <View style={{ marginTop: spacing.xs }}>
                  <Text style={[styles.textFaint, { fontSize: 11, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.5 }]}>output</Text>
                  <Text style={[styles.mono, { marginTop: 2 }]}>{tool.raw_output}</Text>
                </View>
              ) : null}
            </View>
          ) : null}

          {running && tool.can_stop && onStop ? (
            <Pressable
              style={({ pressed }) => [styles.buttonGhost, { marginTop: spacing.md, alignSelf: "flex-start", opacity: pressed ? 0.7 : 1 }]}
              onPress={() => onStop(tool.call_id)}
            >
              <Text style={[styles.text, { color: colors.danger, fontWeight: "600" }]}>Stop</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

export const ToolCallCard = memo(ToolCallCardImpl);
// end of file