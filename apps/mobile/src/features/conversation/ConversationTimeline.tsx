import { memo, useCallback, useRef } from "react";
import { View, Text, FlatList, ActivityIndicator, StyleSheet } from "react-native";
import type { UiSnapshot, ChatMessage } from "../../types";
import { MarkdownBody } from "./MarkdownBody";
import { ToolCallCard } from "../tooling/ToolCallCard";
import { styles, colors, spacing, radius } from "../theme";

interface Props {
  snapshot: UiSnapshot;
  onStopTool?: (toolCallId: string) => void;
}

// Interleaves messages and tool calls chronologically along the timeline, the
// same shape the desktop ConversationTimeline renders. Message ids and tool
// ids are resolved against the snapshot's messages/tools arrays.
function ConversationTimelineImpl({ snapshot, onStopTool }: Props) {
  const listRef = useRef<FlatList<{ key: string; node: React.ReactNode }>>(null);
  const messageById = new Map(snapshot.messages.map((message) => [message.id, message]));
  const toolById = new Map(snapshot.tools.map((tool) => [tool.call_id, tool]));

  const rows = snapshot.timeline.map((item, index) => {
    let node: React.ReactNode;
    if (item === "Thinking") {
      node = (
        <View style={[timelineStyles.thinking, { alignSelf: "flex-start" }]}>
          <ActivityIndicator color={colors.accent} size="small" />
          <Text style={timelineStyles.thinkingText}>{"thinking\u2026"}</Text>
        </View>
      );
    } else if ("Message" in item) {
      const message = messageById.get(item.Message);
      node = message ? <MessageBubble message={message} /> : <Text style={styles.textDim}>(missing message)</Text>;
    } else {
      const tool = toolById.get(item.Tool);
      node = tool ? <ToolCallCard tool={tool} onStop={onStopTool} /> : <Text style={styles.textDim}>(missing tool)</Text>;
    }
    return { key: `${index}`, node };
  });

  const scrollToEnd = useCallback(() => {
    requestAnimationFrame(() => {
      listRef.current?.scrollToEnd({ animated: false });
    });
  }, []);

  return (
    <FlatList
      ref={listRef}
      style={{ flex: 1 }}
      contentContainerStyle={{ padding: spacing.sm, paddingBottom: spacing.xl }}
      data={rows}
      keyExtractor={(row) => row.key}
      renderItem={({ item }) => <View>{item.node}</View>}
      ItemSeparatorComponent={() => <View style={{ height: spacing.sm }} />}
      onContentSizeChange={scrollToEnd}
      ListEmptyComponent={
        <View style={styles.center}>
          <Text style={styles.textDim}>No messages yet.</Text>
        </View>
      }
    />
  );
}

// Message bubble: user messages align right with the accent tint; assistant
// messages align left on a raised surface. Steer messages are dimmed.
function MessageBubble({ message }: { message: ChatMessage }) {
  const isUser = message.role === "User";
  const isSteer = !!message.is_steer;
  return (
    <View style={[timelineStyles.bubbleWrap, isUser ? { alignItems: "flex-end" } : { alignItems: "flex-start" }]}>
      <View
        style={[
          timelineStyles.bubble,
          isUser ? timelineStyles.bubbleUser : timelineStyles.bubbleAssistant,
          isSteer && { opacity: 0.6 },
        ]}
      >
        {!isUser ? (
          <Text style={timelineStyles.roleLabel}>Assistant</Text>
        ) : null}
        <MarkdownBody body={message.body} />
      </View>
    </View>
  );
}

const timelineStyles = StyleSheet.create({
  bubbleWrap: { width: "100%" },
  bubble: { maxWidth: "88%", borderRadius: radius.lg, paddingHorizontal: spacing.md, paddingVertical: spacing.sm + 2, marginTop: spacing.xs },
  bubbleUser: { backgroundColor: colors.accentDim, borderBottomRightRadius: radius.sm, borderWidth: 1, borderColor: "rgba(91,140,255,0.25)" },
  bubbleAssistant: { backgroundColor: colors.surface, borderBottomLeftRadius: radius.sm, borderWidth: 1, borderColor: colors.border },
  roleLabel: { color: colors.accent, fontSize: 10, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.6, marginBottom: 4 },
  thinking: { flexDirection: "row", alignItems: "center", gap: spacing.sm, backgroundColor: colors.surfaceAlt, borderRadius: radius.pill, paddingVertical: spacing.sm, paddingHorizontal: spacing.md, marginTop: spacing.xs, borderWidth: 1, borderColor: colors.border },
  thinkingText: { color: colors.textDim, fontSize: 13, fontStyle: "italic" },
});

export const ConversationTimeline = memo(ConversationTimelineImpl);
// end of file