import { memo, useCallback, useRef, useState } from "react";
import { View, Text, FlatList, Pressable, StyleSheet } from "react-native";
import type { UiSnapshot, ChatMessage, ToolInvocation } from "../../types";
import { MarkdownBody } from "./MarkdownBody";
import { ToolCallCard } from "../tooling/ToolCallCard";
import { ThinkingIndicator } from "../ui/ThinkingIndicator";
import { EmptyState } from "../ui/EmptyState";
import { styles, colors, spacing, radius } from "../theme";

interface Props {
  snapshot: UiSnapshot;
  onStopTool?: (toolCallId: string) => void;
}

const NEAR_BOTTOM_THRESHOLD = 80;

// Interleaves messages and tool calls chronologically along the timeline, the
// same shape the desktop ConversationTimeline renders. Message ids and tool
// ids are resolved against the snapshot's messages/tools arrays.
//
// Auto-follow: while streaming, content growth fires onContentSizeChange
// constantly. We only chase the bottom when the user is already there (or
// hasn't scrolled yet); otherwise a "jump to latest" pill appears so reading
// history is never hijacked by incoming tokens.
function ConversationTimelineImpl({ snapshot, onStopTool }: Props) {
  const listRef = useRef<FlatList<{ key: string; node: React.ReactNode }>>(null);
  const atBottomRef = useRef(true);
  const [showJump, setShowJump] = useState(false);
  const messageById = new Map(snapshot.messages.map((message) => [message.id, message]));
  // Timeline items reference tools by `tool.id` (mirrors the desktop
  // ConversationTimeline, which indexes snapshot.tools by id). `call_id` is a
  // different identifier (used for permission requests / stop_tool), so keying
  // by it here made every tool card render as "(missing tool)". Keep a
  // call_id fallback for robustness against older relay payloads.
  const toolById = new Map<string, ToolInvocation>();
  for (const tool of snapshot.tools) {
    toolById.set(tool.id, tool);
    if (tool.call_id && !toolById.has(tool.call_id)) toolById.set(tool.call_id, tool);
  }

  const turnActive =
    snapshot.session.status === "Streaming" || snapshot.session.status === "WaitingForTool";

  const rows: { key: string; node: React.ReactNode }[] = [];
  snapshot.timeline.forEach((item, index) => {
    if (item === "Thinking") {
      // Desktop renders nothing for Thinking rows; the live indicator only
      // makes sense while the turn is actually running and the marker is the
      // latest timeline entry — otherwise the animation keeps blinking after
      // the session goes idle. Stale Thinking rows are skipped entirely.
      const isLast = index === snapshot.timeline.length - 1;
      if (!turnActive || !isLast) return;
      rows.push({ key: `${index}`, node: <ThinkingIndicator /> });
      return;
    }
    let node: React.ReactNode;
    if ("Message" in item) {
      const message = messageById.get(item.Message);
      node = message ? <MessageBubble message={message} /> : <Text style={styles.textDim}>(missing message)</Text>;
    } else {
      const tool = toolById.get(item.Tool);
      node = tool ? <ToolCallCard tool={tool} onStop={onStopTool} /> : <Text style={styles.textDim}>(missing tool)</Text>;
    }
    rows.push({ key: `${index}`, node });
  });

  const handleScroll = useCallback((event: { nativeEvent: { contentOffset: { y: number }; contentSize: { height: number }; layoutMeasurement: { height: number } } }) => {
    const { contentOffset, contentSize, layoutMeasurement } = event.nativeEvent;
    const distanceFromBottom = contentSize.height - contentOffset.y - layoutMeasurement.height;
    const nearBottom = distanceFromBottom < NEAR_BOTTOM_THRESHOLD;
    atBottomRef.current = nearBottom;
    setShowJump(!nearBottom && contentSize.height > layoutMeasurement.height + 40);
  }, []);

  const jumpToLatest = useCallback(() => {
    setShowJump(false);
    requestAnimationFrame(() => {
      listRef.current?.scrollToEnd({ animated: true });
    });
  }, []);

  const scrollToEnd = useCallback(() => {
    requestAnimationFrame(() => {
      if (atBottomRef.current) listRef.current?.scrollToEnd({ animated: false });
    });
  }, []);

  return (
    <View style={{ flex: 1 }}>
      <FlatList
        ref={listRef}
        style={{ flex: 1 }}
        contentContainerStyle={{ padding: spacing.md, paddingBottom: spacing.xl }}
        data={rows}
        keyExtractor={(row) => row.key}
        renderItem={({ item }) => <View>{item.node}</View>}
        ItemSeparatorComponent={() => <View style={{ height: spacing.sm }} />}
        onScroll={handleScroll}
        scrollEventThrottle={120}
        onContentSizeChange={scrollToEnd}
        removeClippedSubviews
        initialNumToRender={12}
        maxToRenderPerBatch={8}
        windowSize={21}
        ListEmptyComponent={
          <EmptyState glyph={"\u2728"} title="No messages yet." hint="Send a prompt below to get the agent started." />
        }
      />
      {showJump ? (
        <Pressable
          style={({ pressed }) => [timelineStyles.jumpPill, { opacity: pressed ? 0.85 : 1 }]}
          onPress={jumpToLatest}
          accessibilityRole="button"
          accessibilityLabel="Jump to latest"
        >
          <Text style={timelineStyles.jumpText}>{"\u2193 latest"}</Text>
        </Pressable>
      ) : null}
    </View>
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
  jumpPill: {
    position: "absolute",
    bottom: spacing.md,
    alignSelf: "center",
    backgroundColor: colors.surfaceRaised,
    borderRadius: radius.pill,
    paddingVertical: spacing.xs + 1,
    paddingHorizontal: spacing.md,
    borderWidth: 1,
    borderColor: colors.borderStrong,
    shadowColor: "#000",
    shadowOpacity: 0.4,
    shadowRadius: 10,
    shadowOffset: { width: 0, height: 4 },
    elevation: 5,
  },
  jumpText: { color: colors.accentBright, fontSize: 12, fontWeight: "700" },
});

export const ConversationTimeline = memo(ConversationTimelineImpl);
// end of file