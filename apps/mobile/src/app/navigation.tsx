import { NavigationContainer } from "@react-navigation/native";
import { createNativeStackNavigator } from "@react-navigation/native-stack";
import { StatusBar } from "expo-status-bar";
import { useEffect, useState } from "react";
import { View, Text, ActivityIndicator, Pressable } from "react-native";
import { SafeAreaProvider, SafeAreaView } from "react-native-safe-area-context";
import { PairingScreen } from "../features/pairing/PairingScreen";
import { SessionListScreen } from "../features/session-list/SessionListScreen";
import { ConversationScreen } from "../features/conversation/ConversationScreen";
import { SettingsScreen } from "../features/settings/SettingsScreen";
import { DiagnosticsScreen } from "../features/settings/DiagnosticsScreen";
import { AppServicesProvider, useConnectionState } from "./AppServicesContext";
import { colors, spacing } from "../features/theme";

export type RootStackParamList = {
  Sessions: undefined;
  Conversation: { sessionId: string; title: string };
  Settings: undefined;
  Diagnostics: undefined;
};

const Stack = createNativeStackNavigator<RootStackParamList>();

function MainStack({ onRescan }: { onRescan: () => void }) {
  return (
    <Stack.Navigator
      screenOptions={{
        headerStyle: { backgroundColor: colors.surface },
        headerTintColor: colors.text,
        headerTitleStyle: { color: colors.text, fontWeight: "700", fontSize: 17 },
        headerShadowVisible: false,
        contentStyle: { backgroundColor: colors.bg },
      }}
    >
      <Stack.Screen
        name="Sessions"
        options={({ navigation }) => ({
          title: "Maju",
          headerRight: () => (
            <Pressable
              onPress={() => navigation.navigate("Settings")}
              hitSlop={10}
              style={({ pressed }) => ({
                paddingHorizontal: spacing.sm,
                paddingVertical: spacing.xs,
                borderRadius: 999,
                backgroundColor: pressed ? colors.accentTint : "transparent",
                opacity: pressed ? 0.85 : 1,
              })}
            >
              <Text style={{ color: colors.accent, fontSize: 15, fontWeight: "600" }}>Settings</Text>
            </Pressable>
          ),
        })}
      >
        {({ navigation }) => (
          <SessionListScreen
            onOpenSession={(sessionId, title) => navigation.navigate("Conversation", { sessionId, title })}
            onOpenSettings={() => navigation.navigate("Settings")}
          />
        )}
      </Stack.Screen>
      <Stack.Screen
        name="Conversation"
        options={({ route }) => ({ title: route.params.title })}
      >
        {({ route, navigation }) => (
          <ConversationScreen
            key={route.params.sessionId}
            sessionId={route.params.sessionId}
            title={route.params.title}
            onBack={() => navigation.navigate("Sessions")}
          />
        )}
      </Stack.Screen>
      <Stack.Screen name="Settings" options={{ title: "Settings" }}>
        {({ navigation }) => (
          <SettingsScreen
            onRescan={onRescan}
            onOpenDiagnostics={() => navigation.navigate("Diagnostics")}
          />
        )}
      </Stack.Screen>
      <Stack.Screen name="Diagnostics" options={{ title: "Diagnostics" }}>
        {() => <DiagnosticsScreen />}
      </Stack.Screen>
    </Stack.Navigator>
  );
}

// Root: shows pairing until connected, then the main session stack. The driver
// bootstrap (identity load) runs from the AppServicesProvider on mount.
function Root() {
  const connState = useConnectionState();
  const [everConnected, setEverConnected] = useState(false);
  const [showDiagnostics, setShowDiagnostics] = useState(false);

  useEffect(() => {
    if (connState === "connected") setEverConnected(true);
  }, [connState]);

  const showPairing = connState === "disconnected" && !everConnected;
  const showBooting = !showPairing && connState !== "connected" && !everConnected;
  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: colors.bg }}>
      {showPairing ? (
        <PairingScreen onOpenDiagnostics={() => setShowDiagnostics(true)} />
      ) : showBooting ? (
        <View style={{ flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: spacing.xl }}>
          <ActivityIndicator size="large" color={colors.accent} />
          <Text style={{ color: colors.text, fontSize: 16, fontWeight: "600", marginTop: spacing.lg }}>Connecting…</Text>
          <Text style={{ color: colors.textDim, fontSize: 13, marginTop: spacing.xs }}>Establishing end-to-end relay</Text>
          <Pressable
            style={({ pressed }) => ({
              marginTop: spacing.xxl,
              paddingHorizontal: spacing.lg,
              paddingVertical: spacing.sm,
              borderRadius: 999,
              borderWidth: 1,
              borderColor: colors.borderStrong,
              backgroundColor: pressed ? colors.surfaceAlt : "transparent",
            })}
            onPress={() => setShowDiagnostics(true)}
          >
            <Text style={{ color: colors.accent, fontSize: 14, fontWeight: "600" }}>View diagnostics log</Text>
          </Pressable>
        </View>
      ) : (
        <MainStack onRescan={() => setEverConnected(false)} />
      )}
      {showDiagnostics ? (
        <View style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0, backgroundColor: colors.scrim }}>
          <DiagnosticsScreen onClose={() => setShowDiagnostics(false)} />
        </View>
      ) : null}
    </SafeAreaView>
  );
}

export function Navigation() {
  return (
    <SafeAreaProvider>
      <NavigationContainer>
        <StatusBar style="light" />
        <AppServicesProvider>
          <Root />
        </AppServicesProvider>
      </NavigationContainer>
    </SafeAreaProvider>
  );
}
// end of file
