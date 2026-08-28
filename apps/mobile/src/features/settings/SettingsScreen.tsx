import { View, Text, Pressable, Alert } from "react-native";
import { useAppController, useConnectionState, useSubscriptionState } from "../../app/AppServicesContext";
import { styles, colors, spacing, radius } from "../theme";

// Settings: connection state, device id, subscription status, unbind/re-pair,
// and a kill switch (disconnect). All state comes from the controller.
export function SettingsScreen({
  onRescan,
  onOpenDiagnostics,
}: {
  onRescan: () => void;
  onOpenDiagnostics?: () => void;
}) {
  const controller = useAppController();
  const connState = useConnectionState();
  const subscription = useSubscriptionState();

  const unbind = () => {
    Alert.alert(
      "Unbind all machines?",
      "This clears every bound machine from this phone. You will need to re-scan a QR code to pair again.",
      [
        { text: "Cancel", style: "cancel" },
        {
          text: "Unbind all",
          style: "destructive",
          onPress: async () => {
            await controller.unbindAndClear();
            await controller.disconnect();
            onRescan();
          },
        },
      ],
    );
  };

  const kill = async () => {
    await controller.disconnect();
    onRescan();
  };

  const subStatus = subscription.active
    ? `active \u00b7 ${subscription.plan ?? "\u2014"}`
    : "free / inactive";
  const connTint =
    connState === "connected"
      ? { color: colors.success, bg: colors.successTint, border: colors.success }
      : { color: colors.warn, bg: colors.warnTint, border: colors.warn };

  return (
    <View style={styles.screen}>
      <View style={{ padding: spacing.lg }}>
        <Text style={styles.title}>Settings</Text>
        <Text style={styles.subtitle}>Manage the link between this device and your Maju PC.</Text>

        <Text style={styles.sectionHeader}>Connection</Text>
        <View style={styles.card}>
          <View style={styles.rowBetween}>
            <Text style={styles.textDim}>Status</Text>
            <View style={[styles.chip, { backgroundColor: connTint.bg, borderColor: connTint.border }]}>
              <Text style={{ color: connTint.color, fontSize: 11, fontWeight: "700" }}>{connState}</Text>
            </View>
          </View>
        </View>

        <Text style={styles.sectionHeader}>Device</Text>
        <View style={styles.card}>
          <Text style={styles.textDim}>Device id</Text>
          <Text style={[styles.mono, { marginTop: spacing.xs, fontSize: 12, color: colors.text }]}>
            {controller.deviceIdValue ?? "(generating)"}
          </Text>
        </View>

        <Text style={styles.sectionHeader}>Subscription</Text>
        <View style={styles.card}>
          <Text style={styles.textDim}>Plan</Text>
          <Text style={[styles.text, { marginTop: spacing.xs, fontWeight: "600" }]}>{subStatus}</Text>
          {subscription.expiresAt ? (
            <Text style={[styles.textFaint, { marginTop: spacing.xs }]}>
              {`expires ${new Date(subscription.expiresAt).toLocaleDateString()}`}
            </Text>
          ) : null}
        </View>

        <Pressable
          style={({ pressed }) => [styles.buttonGhost, { marginTop: spacing.lg, borderColor: colors.danger, opacity: pressed ? 0.7 : 1 }]}
          onPress={unbind}
        >
          <Text style={[styles.text, { color: colors.danger, fontWeight: "600" }]}>Unbind all machines</Text>
        </Pressable>
        {onOpenDiagnostics ? (
          <Pressable
            style={({ pressed }) => [styles.buttonGhost, { marginTop: spacing.sm, opacity: pressed ? 0.7 : 1 }]}
            onPress={onOpenDiagnostics}
          >
            <Text style={[styles.text, { fontWeight: "600" }]}>Diagnostics log</Text>
          </Pressable>
        ) : null}
        <Pressable
          style={({ pressed }) => [styles.buttonDanger, { marginTop: spacing.sm, opacity: pressed ? 0.9 : 1 }]}
          onPress={kill}
        >
          <Text style={styles.buttonText}>Disconnect (kill switch)</Text>
        </Pressable>
      </View>
    </View>
  );
}
// end of file