import { useState } from "react";
import { View, Text, TextInput, Pressable, ActivityIndicator, ScrollView, Platform } from "react-native";
import { CameraView, useCameraPermissions, scanFromURLAsync } from "expo-camera";
import type { BarcodeScanningResult } from "expo-camera";
import * as ImagePicker from "expo-image-picker";
import { useAppController, useConnectionState } from "../../app/AppServicesContext";
import { WsTransport } from "../../relay/transport";
import { parsePairingQr } from "../../pairing/qr-parse";
import { styles, colors, spacing } from "../theme";

type Phase = "idle" | "dialing" | "authenticating" | "pairing" | "connected" | "error";

// Pairing: scan the PC's QR (relay_endpoint + pairing_code + pc_device_pubkey),
// dial the relay over TLS WebSocket, run DeviceAuth + the E2E handshake, and
// resync state. Manual entry accepts the raw QR JSON as a fallback. Free-tier
// pairing needs no account; a successful pair lands on the session list.
export function PairingScreen({
  onOpenDiagnostics,
}: {
  onOpenDiagnostics?: () => void;
}) {
  const controller = useAppController();
  const connState = useConnectionState();
  const [permission, requestPermission] = useCameraPermissions();
  const [manual, setManual] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);

  async function pairFromJson(qrJson: string) {
    setError(null);
    try {
      const allowInsecure =
        process.env.EXPO_PUBLIC_RELAY_ALLOW_INSECURE_WS === "1";
      const qr = parsePairingQr(qrJson, allowInsecure);
      setPhase("dialing");
      const transport = new WsTransport(qr.relay_endpoint);
      await transport.ready;
      setPhase("authenticating");
      await controller.pairFromTransport(transport, qrJson, allowInsecure);
      setPhase("connected");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  function onBarcodeScanned(result: BarcodeScanningResult) {
    if (phase === "dialing" || phase === "authenticating" || phase === "pairing") return;
    if (!result.data) return;
    void pairFromJson(result.data);
  }

  async function pickQrImage() {
    if (busy) return;
    setError(null);
    try {
      const perm = await ImagePicker.requestMediaLibraryPermissionsAsync();
      if (!perm.granted) {
        setError("photo permission denied");
        setPhase("error");
        return;
      }
      const result = await ImagePicker.launchImageLibraryAsync({
        mediaTypes: ["images"],
        allowsEditing: false,
        quality: 1,
      });
      if (result.canceled || !result.assets?.length) return;
      const uri = result.assets[0].uri;
      const scanned = await scanFromURLAsync(uri, ["qr"]);
      if (!scanned.length) {
        setError("no QR code found in image");
        setPhase("error");
        return;
      }
      void pairFromJson(scanned[0].data);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }
  const busy = phase === "dialing" || phase === "authenticating" || phase === "pairing" || connState === "connecting" || connState === "authenticating" || connState === "paired/e2e";

  return (
    <ScrollView style={styles.screen} contentContainerStyle={{ padding: spacing.md }}>
      <Text style={styles.title}>Pair with Maju PC</Text>
      <Text style={styles.subtitle}>
        Scan the QR code shown on the PC, or paste the pairing payload below.
        Pairing uses end-to-end encryption (X25519 + ChaCha20-Poly1305).
      </Text>

      {!permission || permission.status !== "granted" ? (
        <View style={styles.card}>
          <Text style={styles.text}>Camera permission required to scan the QR.</Text>
          <Pressable style={[styles.button, { marginTop: spacing.md }]} onPress={() => requestPermission()}>
            <Text style={styles.buttonText}>Grant camera</Text>
          </Pressable>
        </View>
      ) : (
        <View style={[styles.card, { padding: 0, overflow: "hidden", height: 260 }]}>
          <CameraView
            facing="back"
            barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
            onBarcodeScanned={onBarcodeScanned}
            style={{ flex: 1 }}
          />
        </View>
      )}

      <Text style={[styles.sectionHeader, { marginTop: spacing.lg }]}>Or upload QR image</Text>
      <Pressable
        style={[styles.button, { marginTop: spacing.sm }, busy && { opacity: 0.5 }]}
        disabled={busy}
        onPress={pickQrImage}
      >
        <Text style={styles.buttonText}>Pick QR from photos</Text>
      </Pressable>

      <Text style={[styles.sectionHeader, { marginTop: spacing.lg }]}>Or paste payload</Text>
      <TextInput
        style={[styles.input, { minHeight: 80 }]}
        placeholder='{"relay_endpoint":"wss://…","pairing_code":"…","pc_device_pubkey":"…"}'
        placeholderTextColor={colors.textDim}
        value={manual}
        onChangeText={setManual}
        multiline
        autoCapitalize="none"
        autoCorrect={false}
      />
      <Pressable
        style={[styles.button, { marginTop: spacing.sm }, manual.trim().length === 0 && { opacity: 0.5 }]}
        disabled={busy || manual.trim().length === 0}
        onPress={() => pairFromJson(manual.trim())}
      >
        <Text style={styles.buttonText}>Pair</Text>
      </Pressable>

      <View style={{ marginTop: spacing.lg, alignItems: "center" }}>
        {busy && <ActivityIndicator color={colors.accent} />}
        <Text style={styles.status}>{phase === "idle" ? `state: ${connState}` : `phase: ${phase}`}</Text>
        {error && (
          <Text style={[styles.textDim, { color: colors.danger, marginTop: spacing.xs }]}>{error}</Text>
        )}
        {phase === "connected" && (
          <Text style={[styles.text, { color: colors.success, marginTop: spacing.sm }]}>
            Paired — end-to-end secure.
          </Text>
        )}
        {Platform.OS !== "web" && (
          <Text style={[styles.textDim, { marginTop: spacing.md, textAlign: "center" }]}>
            {controller.deviceIdValue ? `device: ${controller.deviceIdValue.slice(0, 12)}…` : "generating device id…"}
          </Text>
        )}
        {onOpenDiagnostics ? (
          <Pressable style={{ marginTop: spacing.md, padding: spacing.sm }} onPress={onOpenDiagnostics}>
            <Text style={[styles.text, { color: colors.accent }]}>View diagnostics log</Text>
          </Pressable>
        ) : null}
      </View>
    </ScrollView>
  );
}
// end of file
