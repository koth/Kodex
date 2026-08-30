import { useCallback, useEffect, useState } from "react";
import {
  View,
  Text,
  FlatList,
  Pressable,
  ActivityIndicator,
  Alert,
  StyleSheet,
} from "react-native";
import { useAppController, useConnectionState } from "../../app/AppServicesContext";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { BoundDevice } from "../../account/binding";
import { PairingScreen } from "../pairing/PairingScreen";
import { EmptyState } from "../ui/EmptyState";
import { styles, colors, spacing, radius, shadows } from "../theme";

// Machines: the landing screen. Shows every bound PC (one record per scanned
// QR — the relay keeps one pairing per scan, so several machines can be bound
// to this phone). Tapping a machine resumes ITS pairing (fresh E2E handshake
// with that machine's pairing token + static key) and lands on the session
// list; long-pressing offers unbind; "Add PC" opens the QR scanner.
// Auto-connect on launch was deliberately removed: the user picks a machine.

function shortPeerId(peerDeviceId: string): string {
  return peerDeviceId.slice(0, 10);
}

function relayHost(endpoint: string | undefined): string | null {
  if (!endpoint) return null;
  try {
    return new URL(endpoint).host;
  } catch {
    return null;
  }
}

function boundDate(boundAt: number | undefined): string | null {
  if (!boundAt) return null;
  const parsed = new Date(boundAt);
  if (Number.isNaN(parsed.getTime())) return null;
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function machineLabel(device: BoundDevice): string {
  if (device.label && device.label.trim().length > 0) return device.label.trim();
  return `PC ${shortPeerId(device.peer_device_id)}`;
}

function avatarTint(seed: string): string {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  const palette = ["#5b8cff", "#8b5cf6", "#ec4899", "#f59e0b", "#10b981", "#06b6d4", "#f43f5e", "#a855f7"];
  return palette[h % palette.length];
}

export function MachinesScreen({
  onOpenDiagnostics,
}: {
  onOpenDiagnostics?: () => void;
}) {
  const controller = useAppController();
  const connState = useConnectionState();
  const insets = useSafeAreaInsets();
  const [devices, setDevices] = useState<BoundDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [adding, setAdding] = useState(false);
  const [connectingTo, setConnectingTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await controller.listMachines();
      setDevices(list);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [controller]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const connecting = connectingTo !== null && connState !== "disconnected";
  const busy = connectingTo !== null;

  const connect = useCallback(
    async (device: BoundDevice) => {
      if (busy) return;
      setConnectingTo(device.peer_device_id);
      setError(null);
      try {
        // On success the controller reaches "connected" and the root swaps
        // to the main session stack; no local state to update here.
        await controller.connectToBoundDevice(device.peer_device_id);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setConnectingTo(null);
      }
    },
    [busy, controller],
  );

  const confirmUnbind = useCallback(
    (device: BoundDevice) => {
      Alert.alert(
        "解除绑定该设备？",
        `${machineLabel(device)} will be removed from this phone. You can pair it again anytime by scanning its QR code.`,
        [
          { text: "Cancel", style: "cancel" },
          {
            text: "解绑",
            style: "destructive",
            onPress: () => {
              void (async () => {
                try {
                  setDevices(await controller.removeMachine(device.peer_device_id));
                } catch (e) {
                  setError(e instanceof Error ? e.message : String(e));
                }
              })();
            },
          },
        ],
      );
    },
    [controller],
  );

  if (adding) {
    return (
      <PairingScreen
        onOpenDiagnostics={onOpenDiagnostics}
        onCancel={() => setAdding(false)}
      />
    );
  }

  return (
    // Headerless landing screen: the root SafeAreaView no longer pads the top
    // (the native stack does that itself), so take the inset here.
    <View style={[styles.screen, { paddingTop: insets.top }]}>
      <View style={{ paddingHorizontal: spacing.lg, paddingTop: spacing.xl, paddingBottom: spacing.md }}>
        <Text style={[styles.title, { marginBottom: spacing.xs }]}>设备</Text>
        <Text style={[styles.subtitle, { marginBottom: 0 }]}>
          已配对的电脑。点按连接并浏览其中的会话。
        </Text>
      </View>
      <View style={styles.hairline} />

      <FlatList
        style={{ flex: 1 }}
        contentContainerStyle={{ paddingTop: spacing.sm, paddingBottom: spacing.xl }}
        data={devices}
        keyExtractor={(item) => item.peer_device_id}
        renderItem={({ item }) => (
          <MachineRow
            device={item}
            connecting={connectingTo === item.peer_device_id && (connState === "connecting" || connState === "authenticating" || connState === "paired/e2e")}
            disabled={busy}
            onConnect={() => void connect(item)}
            onUnbind={() => confirmUnbind(item)}
          />
        )}
        ListEmptyComponent={
          loading ? (
            <View style={styles.center}>
              <ActivityIndicator color={colors.accent} />
            </View>
          ) : (
            <EmptyState
              glyph={"\u2302"}
              title="还没有设备"
              hint="在电脑端展示配对二维码,再点下方按钮扫码。可绑定多台电脑并随时切换。"
            />
          )
        }
        ListFooterComponent={
          error ? (
            <View style={{ padding: spacing.lg }}>
              <Text style={[styles.textFaint, { textAlign: "center", color: colors.danger }]}>{error}</Text>
            </View>
          ) : null
        }
      />

      <View style={{ paddingHorizontal: spacing.lg, paddingBottom: spacing.lg }}>
        <Pressable
          style={({ pressed }) => [
            styles.button,
            { opacity: pressed ? 0.9 : 1 },
            busy && { opacity: 0.5 },
          ]}
          disabled={busy}
          onPress={() => setAdding(true)}
        >
          <Text style={styles.buttonText}>+ 配对新电脑</Text>
        </Pressable>
        {onOpenDiagnostics ? (
          <Pressable
            style={({ pressed }) => ({ alignSelf: "center", marginTop: spacing.md, padding: spacing.sm, opacity: pressed ? 0.7 : 1 })}
            onPress={onOpenDiagnostics}
          >
            <Text style={[styles.text, { color: colors.textDim, fontSize: 13 }]}>查看诊断日志</Text>
          </Pressable>
        ) : null}
      </View>
    </View>
  );
}

function MachineRow({
  device,
  connecting,
  disabled,
  onConnect,
  onUnbind,
}: {
  device: BoundDevice;
  connecting: boolean;
  disabled: boolean;
  onConnect: () => void;
  onUnbind: () => void;
}) {
  const label = machineLabel(device);
  const host = relayHost(device.relay_endpoint);
  const date = boundDate(device.bound_at);
  const metaParts = [host, date].filter(Boolean).join(" \u00b7 ");
  return (
    <Pressable
      style={({ pressed }) => [
        localStyles.row,
        { opacity: pressed ? 0.75 : disabled && !connecting ? 0.55 : 1 },
        connecting && localStyles.rowConnecting,
      ]}
      onPress={onConnect}
      onLongPress={onUnbind}
      disabled={disabled && !connecting}
      accessibilityRole="button"
      accessibilityLabel={`Connect to ${label}`}
    >
      <View style={[localStyles.avatar, { backgroundColor: avatarTint(device.peer_device_id) }]}>
        <Text style={styles.avatarText}>{label.trim()[0]?.toUpperCase() ?? "P"}</Text>
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={localStyles.name} numberOfLines={1}>
          {label}
        </Text>
        <Text style={localStyles.meta} numberOfLines={1}>
          {host ?? "relay endpoint unknown"}
          {date ? ` \u00b7 paired ${date}` : ""}
        </Text>
      </View>
      {connecting ? (
        <View style={localStyles.connectingWrap}>
          <ActivityIndicator color={colors.accent} size="small" />
          <Text style={localStyles.connectingText}>连接中…</Text>
        </View>
      ) : (
        <Pressable
          style={({ pressed }) => [localStyles.unbind, { opacity: pressed ? 0.6 : 1 }]}
          hitSlop={8}
          onPress={(event) => {
            event.stopPropagation();
            onUnbind();
          }}
          disabled={disabled}
          accessibilityRole="button"
          accessibilityLabel={`Unbind ${label}`}
        >
          <Text style={localStyles.unbindText}>解绑</Text>
        </Pressable>
      )}
    </Pressable>
  );
}

const localStyles = StyleSheet.create({
  row: {
    flexDirection: "row",
    alignItems: "center",
    backgroundColor: colors.surface,
    borderRadius: radius.lg,
    borderWidth: 1,
    borderColor: colors.border,
    paddingVertical: spacing.md + 2,
    paddingHorizontal: spacing.md,
    marginHorizontal: spacing.sm,
    marginTop: spacing.xs,
    ...shadows.card,
  },
  rowConnecting: {
    borderColor: colors.accent,
  },
  avatar: {
    width: 40,
    height: 40,
    borderRadius: 12,
    alignItems: "center",
    justifyContent: "center",
    marginRight: spacing.md,
  },
  name: {
    color: colors.text,
    fontSize: 15,
    fontWeight: "700",
    flexShrink: 1,
  },
  meta: {
    color: colors.textFaint,
    fontSize: 12,
    marginTop: 2,
  },
  connectingWrap: {
    flexDirection: "row",
    alignItems: "center",
    marginLeft: spacing.sm,
  },
  connectingText: {
    color: colors.accent,
    fontSize: 12,
    fontWeight: "600",
    marginLeft: spacing.sm,
  },
  unbind: {
    paddingVertical: spacing.xs + 2,
    paddingHorizontal: spacing.md,
    borderRadius: radius.pill,
    borderWidth: 1,
    borderColor: colors.borderStrong,
    backgroundColor: colors.surfaceAlt,
    marginLeft: spacing.sm,
  },
  unbindText: {
    color: colors.danger,
    fontSize: 12,
    fontWeight: "600",
  },
});
// end of file