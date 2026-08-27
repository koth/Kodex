import { useCallback, useEffect, useRef, useState } from "react";
import { View, Text, FlatList, Pressable, ActivityIndicator, RefreshControl, StyleSheet, Animated, Easing } from "react-native";
import { useFocusEffect } from "@react-navigation/native";
import { useAppController, useConnectionState } from "../../app/AppServicesContext";
import type { SessionListItem, WorkspaceSessionList } from "../../types";
import { splitSessionGroups } from "./session-groups";
import { styles, colors, spacing, radius, shadows } from "../theme";
import { EmptyState } from "../ui/EmptyState";

// Lists sessions from `ListSessions`. The project-less chats workspace
// (marked `kind: "chats"`) renders as a pinned "聊天" section with its
// sessions always visible — mirroring the desktop sidebar where chats are a
// first-level group next to projects, not a project itself. Every project
// renders as a collapsible row that starts collapsed (except the currently
// active workspace), and expanding a project reveals its session list.
// Pull-to-refresh re-issues `ListSessions`; the expand/collapse map is
// hoisted here so background refreshes never reset it.

type Group = WorkspaceSessionList;

type Row =
  | { kind: "chats-header"; key: string }
  | { kind: "workspace"; key: string; group: Group }
  | {
      kind: "session";
      key: string;
      session: SessionListItem;
      isSessionActive: boolean;
    };

function sortSessions(sessions: SessionListItem[]): SessionListItem[] {
  return [...sessions].sort((a, b) => {
    return (
      getTimestamp(b.updated_at || b.created_at) -
      getTimestamp(a.updated_at || a.created_at)
    );
  });
}

function getTimestamp(value: string | undefined): number {
  if (!value) return 0;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

function formatRelativeTime(value: string | undefined): string | null {
  const timestamp = getTimestamp(value);
  if (!timestamp) return null;
  const diffMs = Date.now() - timestamp;
  const minutes = Math.max(0, Math.floor(diffMs / 60000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  if (hours < 48) return "yesterday";
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  return new Date(timestamp).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

function statusLabel(status: string): string {
  switch (status) {
    case "Streaming":
    case "WaitingForTool":
      return "running";
    case "Interrupted":
      return "interrupted";
    default:
      return "idle";
  }
}

function statusTint(status: string): { color: string; bg: string; border: string } {
  switch (status) {
    case "Streaming":
    case "WaitingForTool":
      return { color: colors.success, bg: colors.successTint, border: colors.success };
    case "Interrupted":
      return { color: colors.danger, bg: colors.dangerTint, border: colors.danger };
    default:
      return { color: colors.textDim, bg: colors.surfaceAlt, border: colors.border };
  }
}

function avatarGradientColor(name: string): string {
  // Deterministic accent-ish hue per project so avatars read distinct.
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  const palette = ["#5b8cff", "#8b5cf6", "#ec4899", "#f59e0b", "#10b981", "#06b6d4", "#f43f5e", "#a855f7"];
  return palette[h % palette.length];
}

export function SessionListScreen({
  onOpenSession,
  onOpenSettings,
}: {
  onOpenSession: (sessionId: string, title: string) => void;
  onOpenSettings: () => void;
}) {
  const controller = useAppController();
  const connState = useConnectionState();
  const [groups, setGroups] = useState<Group[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creatingFor, setCreatingFor] = useState<string | "global" | null>(null);

  // Per-workspace expand state keyed by workspace root. Unset entries fall
  // back to "expanded iff this is the active workspace" so the list mirrors
  // the desktop sidebar's default-collapsed projects.
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await controller.listSessions();
      setGroups(res.sessions);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [controller]);

  useEffect(() => {
    if (connState === "connected") void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connState]);

  useFocusEffect(
    useCallback(() => {
      if (connState === "connected") void refresh();
    }, [connState, refresh]),
  );

  const connected = connState === "connected";

  const toggleWorkspace = useCallback((root: string) => {
    setExpanded((current) => ({ ...current, [root]: !(current[root] ?? false) }));
  }, []);

  const createInWorkspace = useCallback(
    async (group: Group) => {
      if (!connected || group.workspace.location?.kind === "remote_linux") return;
      setCreatingFor(group.workspace.root);
      setError(null);
      try {
        const id = await controller.createSession({
          workspaceRoot: group.workspace.root,
        });
        onOpenSession(id, "New session");
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setCreatingFor(null);
      }
    },
    [connected, controller, onOpenSession],
  );

  // Flatten groups into section rows. Chats render first as a pinned, always
  // expanded section; collapsed projects contribute no session rows, so the
  // list stays short like the desktop sidebar.
  const { chats: chatsGroups, projects: projectGroups } = splitSessionGroups(groups);
  const chatsGroup = chatsGroups[0];
  const rows: Row[] = [];
  let anySessionVisible = false;
  if (chatsGroup) {
    rows.push({ kind: "chats-header", key: "chats:header" });
    const sortedChats = sortSessions(chatsGroup.sessions);
    if (sortedChats.length === 0) {
      rows.push({
        kind: "session",
        key: `chats-empty:${chatsGroup.workspace.root}`,
        session: {
          id: "",
          title: chatsGroup.connected ? "No chats yet" : "Not loaded yet",
          status: "Idle",
          created_at: "",
          updated_at: "",
          message_count: 0,
        },
        isSessionActive: false,
      });
    } else {
      for (const session of sortedChats) {
        anySessionVisible = true;
        rows.push({
          kind: "session",
          key: `${chatsGroup.workspace.root}:${session.id}`,
          session,
          isSessionActive:
            chatsGroup.is_active && session.id === chatsGroup.active_session_id,
        });
      }
    }
  }
  for (const group of projectGroups) {
    rows.push({ kind: "workspace", key: `ws:${group.workspace.root}`, group });
    const isOpen = expanded[group.workspace.root] ?? group.is_active;
    if (!isOpen) continue;
    const sorted = sortSessions(group.sessions);
    if (sorted.length === 0) {
      rows.push({
        kind: "session",
        key: `ws-empty:${group.workspace.root}`,
        session: {
          id: "",
          title: group.connected ? "No sessions yet" : "Not loaded yet",
          status: "Idle",
          created_at: "",
          updated_at: "",
          message_count: 0,
        },
        isSessionActive: false,
      });
      continue;
    }
    for (const session of sorted) {
      anySessionVisible = true;
      rows.push({
        kind: "session",
        key: `${group.workspace.root}:${session.id}`,
        session,
        isSessionActive: group.is_active && session.id === group.active_session_id,
      });
    }
  }

  const createGlobal = useCallback(async () => {
    if (!connected) return;
    setCreatingFor("global");
    setError(null);
    try {
      const id = await controller.createSession();
      onOpenSession(id, "New session");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreatingFor(null);
    }
  }, [connected, controller, onOpenSession]);

  return (
    <View style={styles.screen}>
      <View style={[styles.rowBetween, { paddingHorizontal: spacing.lg, paddingVertical: spacing.md }]}>
        <View>
          <Text style={[styles.title, { marginBottom: 0, fontSize: 26 }]}>Projects</Text>
          <Text style={[styles.textFaint, { marginTop: 2 }]}>
            {connected
              ? `${projectGroups.length} workspace${projectGroups.length === 1 ? "" : "s"}`
              : "offline"}
          </Text>
        </View>
        <View style={styles.row}>
          <Pressable
            style={({ pressed }) => [localStyles.headButton, { opacity: pressed ? 0.7 : 1 }]}
            onPress={onOpenSettings}
            hitSlop={8}
          >
            <Text style={[styles.text, { fontSize: 14, fontWeight: "600" }]}>Settings</Text>
          </Pressable>
          <Pressable
            style={({ pressed }) => [
              localStyles.newButton,
              { opacity: pressed ? 0.9 : connected ? 1 : 0.45 },
            ]}
            onPress={createGlobal}
            disabled={creatingFor !== null || !connected}
          >
            {creatingFor === "global" || !connected ? (
              <ActivityIndicator color="#fff" size="small" />
            ) : (
              <Text style={styles.buttonText}>New</Text>
            )}
          </Pressable>
        </View>
      </View>

      <View style={styles.hairline} />

      <FlatList
        style={{ flex: 1 }}
        contentContainerStyle={{ paddingTop: spacing.sm, paddingBottom: spacing.xl }}
        refreshControl={<RefreshControl refreshing={loading} onRefresh={refresh} tintColor={colors.accent} />}
        data={rows}
        keyExtractor={(item) => item.key}
        renderItem={({ item }) =>
          item.kind === "chats-header" ? (
            <ChatsHeader
              creating={chatsGroup ? creatingFor === chatsGroup.workspace.root : false}
              disabled={!chatsGroup || !chatsGroup.connected || !connected}
              onCreate={() => chatsGroup && void createInWorkspace(chatsGroup)}
            />
          ) : item.kind === "workspace" ? (
            <WorkspaceRow
              group={item.group}
              expanded={expanded[item.group.workspace.root] ?? item.group.is_active}
              creating={creatingFor === item.group.workspace.root}
              onToggle={() => toggleWorkspace(item.group.workspace.root)}
              onCreate={() => void createInWorkspace(item.group)}
            />
          ) : (
            <SessionRow
              session={item.session}
              active={item.isSessionActive}
              placeholder={item.session.id === ""}
              onPress={
                item.session.id === ""
                  ? undefined
                  : () => onOpenSession(item.session.id, item.session.title)
              }
            />
          )
        }
        ListEmptyComponent={
          loading ? (
            <View style={styles.center}><ActivityIndicator color={colors.accent} /></View>
          ) : (
              <EmptyState
                glyph={"\u2302"}
                title={error ? "Could not load projects" : "No projects yet"}
                hint={error ? "Pull down to retry." : "Open a workspace on desktop and it will appear here."}
              />
            )
        }
        ListFooterComponent={
          rows.length > 0 && error ? (
            <View style={{ padding: spacing.lg }}>
              <Text style={[styles.textFaint, { textAlign: "center" }]}>{error}</Text>
            </View>
          ) : null
        }
      />
      {!anySessionVisible && groups.length > 0 && !loading ? (
        <Text style={localStyles.hint}>Tap a project to reveal its sessions</Text>
      ) : null}
    </View>
  );
}

// Section header for the pinned chats group: a "聊天" label plus a shortcut
// that starts a new chat in the project-less chats workspace. Kept visually
// lighter than project cards, like the desktop sidebar's chats group.
function ChatsHeader({
  creating,
  disabled,
  onCreate,
}: {
  creating: boolean;
  disabled: boolean;
  onCreate: () => void;
}) {
  return (
    <View style={localStyles.chatsHeader}>
      <Text style={localStyles.chatsKicker}>{"聊天"}</Text>
      <Pressable
        style={({ pressed }) => [
          localStyles.chatsNew,
          { opacity: pressed ? 0.7 : disabled ? 0.4 : 1 },
        ]}
        onPress={onCreate}
        disabled={disabled || creating}
        accessibilityRole="button"
        accessibilityLabel="新建聊天"
        hitSlop={8}
      >
        {creating ? (
          <ActivityIndicator color={colors.accent} size="small" />
        ) : (
          <Text style={localStyles.chatsNewText}>{"+ 新聊天"}</Text>
        )}
      </Pressable>
    </View>
  );
}

function WorkspaceRow({
  group,
  expanded,
  creating,
  onToggle,
  onCreate,
}: {
  group: Group;
  expanded: boolean;
  creating: boolean;
  onToggle: () => void;
  onCreate: () => void;
}) {
  const running = group.sessions.some(
    (s) => s.status === "Streaming" || s.status === "WaitingForTool",
  );
  const remote = group.workspace.location?.kind === "remote_linux";
  const dormant = remote && !group.connected;
  const initial = (group.workspace.name.trim()[0] ?? "?").toUpperCase();
  const tint = avatarGradientColor(group.workspace.name);
  return (
    <View
      style={[
        localStyles.projectCard,
        group.is_active ? { borderColor: colors.accent, ...shadows.card } : null,
      ]}
    >
      <Pressable
        style={({ pressed }) => [localStyles.projectRow, { opacity: pressed ? 0.8 : 1 }]}
        onPress={onToggle}
        accessibilityRole="button"
        accessibilityState={{ expanded }}
      >
        <View style={[localStyles.projectAvatar, { backgroundColor: tint }]}>
          <Text style={styles.avatarText}>{initial}</Text>
        </View>
        <View style={{ flex: 1, minWidth: 0 }}>
          <View style={styles.row}>
            <Text style={localStyles.projectName} numberOfLines={1}>
              {group.workspace.name}
            </Text>
            {running && !dormant ? <View style={localStyles.runningDot} /> : null}
          </View>
          <Text style={localStyles.projectMeta} numberOfLines={1}>
            {group.connected
              ? `${group.sessions.length} session${group.sessions.length === 1 ? "" : "s"}`
              : remote
                ? "remote \u00b7 offline"
                : "offline"}
          </Text>
        </View>
        <AnimatedChevron expanded={expanded} />
        {!remote ? (
          <Pressable
            style={({ pressed }) => [localStyles.plusButton, { opacity: pressed ? 0.6 : 1 }]}
            hitSlop={10}
            onPress={(event) => {
              event.stopPropagation();
              onCreate();
            }}
            disabled={!group.connected || creating}
          >
            {creating ? (
              <ActivityIndicator color={colors.accent} size="small" />
            ) : (
              <Text style={localStyles.plusText}>+</Text>
            )}
          </Pressable>
        ) : null}
      </Pressable>
    </View>
  );
}

// Rotating disclosure arrow (single glyph, rotated 0deg -> 90deg) so the
// expand/collapse affordance animates instead of snapping between glyphs.
function AnimatedChevron({ expanded }: { expanded: boolean }) {
  const spin = useRef(new Animated.Value(expanded ? 1 : 0)).current;
  useEffect(() => {
    Animated.timing(spin, {
      toValue: expanded ? 1 : 0,
      duration: 180,
      easing: Easing.out(Easing.quad),
      useNativeDriver: true,
    }).start();
  }, [expanded, spin]);
  return (
    <Animated.View style={{ transform: [{ rotate: spin.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "90deg"] }) }] }}>
      <Text style={localStyles.chevron}>{"\u203A"}</Text>
    </Animated.View>
  );
}

function SessionRow({
  session,
  active,
  placeholder,
  onPress,
}: {
  session: SessionListItem;
  active: boolean;
  placeholder: boolean;
  onPress?: () => void;
}) {
  const time = formatRelativeTime(session.updated_at || session.created_at);
  const tint = statusTint(session.status);
  return (
    <Pressable
      style={({ pressed }) => [localStyles.sessionRow, active && localStyles.sessionActive, { opacity: pressed ? 0.7 : 1 }]}
      onPress={onPress}
      disabled={!onPress}
    >
      {active ? <View style={localStyles.activeBar} /> : null}
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[styles.text, { fontWeight: "600", fontSize: 15 }]} numberOfLines={1}>
          {session.title}
        </Text>
        {placeholder ? null : (
          <View style={[styles.row, { marginTop: 4 }]}>
            <View style={[styles.chip, { backgroundColor: tint.bg, borderColor: tint.border }]}>
              <Text style={{ color: tint.color, fontSize: 11, fontWeight: "600" }}>{statusLabel(session.status)}</Text>
            </View>
            {time ? <Text style={[styles.textFaint, { marginLeft: spacing.sm }]}>{time}</Text> : null}
          </View>
        )}
      </View>
    </Pressable>
  );
}

const localStyles = StyleSheet.create({
  chatsHeader: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingHorizontal: spacing.lg,
    paddingTop: spacing.sm,
    paddingBottom: spacing.xs,
  },
  chatsKicker: {
    fontSize: 13,
    fontWeight: "700",
    letterSpacing: 1,
    color: colors.textFaint,
  },
  chatsNew: {
    flexDirection: "row",
    alignItems: "center",
    paddingVertical: spacing.xs + 2,
    paddingHorizontal: spacing.md,
    borderRadius: radius.pill,
    borderWidth: 1,
    borderColor: colors.border,
    backgroundColor: colors.surface,
  },
  chatsNewText: {
    fontSize: 13,
    fontWeight: "600",
    color: colors.text,
  },
  headButton: {
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
    borderRadius: radius.pill,
    borderWidth: 1,
    borderColor: colors.borderStrong,
    backgroundColor: colors.surface,
    marginRight: spacing.sm,
  },
  newButton: {
    backgroundColor: colors.accent,
    borderRadius: radius.pill,
    paddingVertical: spacing.sm + 1,
    paddingHorizontal: spacing.lg + 4,
    alignItems: "center",
    justifyContent: "center",
    minWidth: 56,
    minHeight: 36,
    ...shadows.glow,
  },
  projectCard: {
    backgroundColor: colors.surface,
    borderRadius: radius.lg,
    marginHorizontal: spacing.sm,
    marginTop: spacing.xs,
    marginBottom: spacing.xs,
    borderWidth: 1,
    borderColor: colors.border,
    overflow: "hidden",
  },
  projectRow: {
    flexDirection: "row",
    alignItems: "center",
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.md,
  },
  projectAvatar: {
    width: 38,
    height: 38,
    borderRadius: 12,
    alignItems: "center",
    justifyContent: "center",
    marginRight: spacing.md,
  },
  projectName: {
    color: colors.text,
    fontSize: 15,
    fontWeight: "700",
    flexShrink: 1,
  },
  projectMeta: {
    color: colors.textFaint,
    fontSize: 12,
    marginTop: 2,
  },
  chevron: {
    color: colors.textDim,
    fontSize: 13,
    width: 16,
    marginLeft: spacing.sm,
  },
  runningDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    backgroundColor: colors.success,
    marginLeft: spacing.sm,
  },
  plusButton: {
    width: 30,
    height: 30,
    borderRadius: 10,
    alignItems: "center",
    justifyContent: "center",
    borderWidth: 1,
    borderColor: colors.borderStrong,
    backgroundColor: colors.surfaceAlt,
    marginLeft: spacing.sm,
  },
  plusText: {
    color: colors.text,
    fontSize: 18,
    lineHeight: 20,
  },
  sessionRow: {
    flexDirection: "row",
    alignItems: "center",
    backgroundColor: colors.surfaceAlt,
    borderRadius: radius.md,
    paddingVertical: spacing.sm + 2,
    paddingHorizontal: spacing.md,
    marginHorizontal: spacing.md,
    marginTop: spacing.xs,
    borderWidth: 1,
    borderColor: "transparent",
  },
  sessionActive: {
    borderColor: colors.accent,
    backgroundColor: colors.surface,
  },
  activeBar: {
    width: 4,
    height: 26,
    borderRadius: 2,
    backgroundColor: colors.accent,
    marginRight: spacing.sm,
  },
  hint: {
    color: colors.textFaint,
    fontSize: 12,
    textAlign: "center",
    paddingVertical: spacing.sm,
  },
});
// end of file