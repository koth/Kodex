// On-device diagnostics viewer: shows the persisted log (pairing, relay
// I/O, identity persistence) so connection issues can be reported without
// adb/Xcode. Copy button puts the whole log on the clipboard for pasting.
import { useCallback, useEffect, useState } from "react";
import { View, Text, Pressable, ScrollView, Share } from "react-native";
import { diagnostics } from "../../util/diagnostics";
import { styles, colors, spacing } from "../theme";

export function DiagnosticsScreen({ onClose }: { onClose?: () => void }) {
  const [log, setLog] = useState("loading…");

  const refresh = useCallback(async () => {
    const text = await diagnostics.readAll();
    setLog(text || "(no log yet)");
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const share = async () => {
    const text = await diagnostics.readAll();
    await Share.share({ message: text || "(empty)" });
  };

  const clear = async () => {
    await diagnostics.clear();
    await refresh();
  };

  return (
    <View style={styles.screen}>
      <View
        style={{
          flexDirection: "row",
          gap: spacing.sm,
          padding: spacing.md,
        }}
      >
        {onClose ? (
          <Pressable style={styles.buttonGhost} onPress={onClose}>
            <Text style={styles.text}>Close</Text>
          </Pressable>
        ) : null}
        <Pressable style={styles.buttonGhost} onPress={() => void refresh()}>
          <Text style={styles.text}>Refresh</Text>
        </Pressable>
        <Pressable style={styles.buttonGhost} onPress={() => void share()}>
          <Text style={styles.text}>Share</Text>
        </Pressable>
        <Pressable style={styles.buttonGhost} onPress={() => void clear()}>
          <Text style={[styles.text, { color: colors.danger }]}>Clear</Text>
        </Pressable>
      </View>
      <ScrollView style={{ flex: 1, paddingHorizontal: spacing.md }}>
        <Text style={[styles.mono, { fontSize: 11, color: colors.textDim }]}>
          {log}
        </Text>
      </ScrollView>
    </View>
  );
}
