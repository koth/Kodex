import { memo, useState } from "react";
import { View, Text, Pressable, ScrollView, StyleSheet } from "react-native";
import type { TurnFileChanges, UiSnapshot } from "../../types";
import { colors, spacing } from "../theme";

// Pinned above the composer: the files changed by the most recent turn that
// modified any file — the mobile counterpart of the desktop per-turn
// ChangesBar. Collapsed shows one summary line (label + file count + line
// totals); expanding lists every file with its +/- counts. Per-file diffs
// live in the turn's tool cards, so this bar is the turn-level index, not a
// second diff surface.

function latestTurnWithChanges(turns: TurnFileChanges[]): TurnFileChanges | null {
  for (let i = turns.length - 1; i >= 0; i--) {
    if (turns[i].changes.length > 0) return turns[i];
  }
  return null;
}

function lastTimelineMessageId(snapshot: UiSnapshot): string | null {
  for (let i = snapshot.timeline.length - 1; i >= 0; i--) {
    const item = snapshot.timeline[i];
    if (typeof item === "object" && "Message" in item) return item.Message;
  }
  return null;
}

export const TurnChangesBar = memo(function TurnChangesBar({ snapshot }: { snapshot: UiSnapshot }) {
  const [expanded, setExpanded] = useState(false);
  const turn = latestTurnWithChanges(snapshot.turn_changes ?? []);
  if (!turn) return null;

  const files = turn.changes;
  const added = files.reduce((sum, file) => sum + file.added_lines, 0);
  const removed = files.reduce((sum, file) => sum + file.removed_lines, 0);
  // The turn is "current" while its closing assistant reply is still the last
  // timeline entry; once a newer turn starts (even without file changes yet)
  // the bar relabels so it never claims stale work is 本轮.
  const isCurrentTurn = turn.message_id === lastTimelineMessageId(snapshot);

  return (
    <View style={barStyles.wrap}>
      <Pressable
        style={({ pressed }) => [barStyles.header, pressed ? barStyles.headerPressed : null]}
        onPress={() => setExpanded((value) => !value)}
        accessibilityRole="button"
        accessibilityState={{ expanded }}
        accessibilityLabel={expanded ? "收起本轮改动" : "展开本轮改动"}
      >
        <Text style={barStyles.label}>{isCurrentTurn ? "本轮改动" : "上轮改动"}</Text>
        <Text style={barStyles.meta}>{`${files.length} 个文件`}</Text>
        <Text style={[barStyles.count, { color: colors.success }]}>{`+${added}`}</Text>
        <Text style={[barStyles.count, { color: colors.danger }]}>{`\u2212${removed}`}</Text>
        <Text style={[barStyles.chevron, expanded ? barStyles.chevronOpen : null]}>{"\u203A"}</Text>
      </Pressable>
      {expanded ? (
        <ScrollView style={barStyles.list} nestedScrollEnabled>
          {files.map((file, index) => (
            <View key={`${file.path}:${index}`} style={barStyles.fileRow}>
              <Text
                style={[barStyles.path, file.change_type === "Deleted" ? barStyles.pathDeleted : null]}
                numberOfLines={1}
              >
                {file.path}
              </Text>
              <Text style={[barStyles.count, { color: colors.success }]}>{`+${file.added_lines}`}</Text>
              <Text style={[barStyles.count, { color: colors.danger }]}>{`\u2212${file.removed_lines}`}</Text>
            </View>
          ))}
        </ScrollView>
      ) : null}
    </View>
  );
});

const barStyles = StyleSheet.create({
  wrap: {
    borderTopWidth: 1,
    borderTopColor: colors.border,
    backgroundColor: colors.surfaceAlt,
    maxHeight: 240,
  },
  header: {
    flexDirection: "row",
    alignItems: "center",
    paddingVertical: spacing.sm - 2,
    paddingHorizontal: spacing.md,
  },
  headerPressed: {
    backgroundColor: colors.surface,
  },
  label: {
    color: colors.text,
    fontSize: 12,
    fontWeight: "700",
  },
  meta: {
    color: colors.textDim,
    fontSize: 12,
    marginLeft: spacing.sm,
    flexShrink: 1,
  },
  count: {
    fontSize: 12,
    fontWeight: "700",
    fontVariant: ["tabular-nums"],
    marginLeft: spacing.sm,
  },
  chevron: {
    color: colors.textDim,
    fontSize: 13,
    marginLeft: "auto",
    paddingLeft: spacing.sm,
    transform: [{ rotate: "90deg" }],
  },
  chevronOpen: {
    transform: [{ rotate: "-90deg" }],
  },
  list: {
    maxHeight: 200,
    paddingHorizontal: spacing.md,
    paddingBottom: spacing.sm,
  },
  fileRow: {
    flexDirection: "row",
    alignItems: "center",
    paddingVertical: spacing.xs - 1,
    gap: spacing.sm,
  },
  path: {
    color: colors.textDim,
    fontSize: 12,
    flex: 1,
    fontFamily: "monospace",
  },
  pathDeleted: {
    color: colors.textFaint,
    textDecorationLine: "line-through",
  },
});
// end of file
