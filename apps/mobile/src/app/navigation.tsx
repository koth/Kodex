import { NavigationContainer } from "@react-navigation/native";
import { createNativeStackNavigator } from "@react-navigation/native-stack";
import { StatusBar } from "expo-status-bar";
import { useEffect, useState } from "react";
import { View, Text, Pressable } from "react-native";
import { SafeAreaProvider, SafeAreaView } from "react-native-safe-area-context";
import { PairingScreen } from "../features/pairing/PairingScreen";
import { MachinesScreen } from "../features/machines/MachinesScreen";
import { SessionListScreen } from "../features/session-list/SessionListScreen";
import { ConversationScreen } from "../features/conversation/ConversationScreen";
import { SettingsScreen } from "../features/settings/SettingsScreen";
import { DiagnosticsScreen } from "../features/settings/DiagnosticsScreen";
import { AppServicesProvider, useConnectionState, useSnapshot } from "./AppServicesContext";
import { colors, spacing } from "../features/theme";

export type RootStackParamList = {
  Sessions: undefined;
  Conversation: { sessionId: string; title: string };
  Settings: undefined;
  Diagnostics: undefined;
};

const Stack = createNativeStackNavigator<RootStackParamList>();

// Native header title for the conversation screen: the session title with a
// small live/idle status line under it (iOS-subtitle style). Driven by the
// session store so the status tracks the PC without any extra chrome — this
// replaces the status chip the old in-screen header row used to duplicate.
function ConversationHeaderTitle({ title }: { title: string }) {
  const snapshot = useSnapshot();
  const status = snapshot?.session.status ?? "Idle";
  const live = status === "Streaming" || status === "WaitingForTool";
  const tint = live
    ? colors.success
    : status === "Interrupted"
      ? colors.danger
      : colors.textDim;
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center" }}>
      <Text numberOfLines={1} style={{ color: colors.text, fontSize: 16, fontWeight: "700" }}>
        {title}
      </Text>
      <Text
        style={{
          color: tint,
          fontSize: 10,
          fontWeight: "700",
          letterSpacing: 0.5,
          marginTop: 1,
        }}
      >
        {live ? "运行中" : status === "Interrupted" ? "已中断" : "空闲"}
      </Text>
    </View>
  );
}

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
              <Text style={{ color: colors.accent, fontSize: 15, fontWeight: "600" }}>设置</Text>
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
        options={({ route }) => ({
          headerTitle: () => <ConversationHeaderTitle title={route.params.title} />,
        })}
      >
        {({ route }) => (
          <ConversationScreen
            key={route.params.sessionId}
            sessionId={route.params.sessionId}
          />
        )}
      </Stack.Screen>
      <Stack.Screen name="Settings" options={{ title: "设置" }}>
        {({ navigation }) => (
          <SettingsScreen
            onRescan={onRescan}
            onOpenDiagnostics={() => navigation.navigate("Diagnostics")}
          />
        )}
      </Stack.Screen>
      <Stack.Screen name="Diagnostics" options={{ title: "诊断" }}>
        {() => <DiagnosticsScreen />}
      </Stack.Screen>
    </Stack.Navigator>
  );
}

// Root: the machines list is the landing state — one entry per bound PC,
// with add (scan QR) / unbind actions and tap-to-connect. Once a connection
// reaches "connected" the main session stack takes over (`everConnected`
// latches until the user hits the kill switch / re-pair in Settings). The
// driver bootstrap (identity load) runs from the AppServicesProvider on mount;
// boot() deliberately does NOT auto-connect — the user picks the machine.
function Root() {
  const connState = useConnectionState();
  const [everConnected, setEverConnected] = useState(false);
  const [showDiagnostics, setShowDiagnostics] = useState(false);

  useEffect(() => {
    if (connState === "connected") setEverConnected(true);
  }, [connState]);

  return (
    // No top edge here: the native stack header insets the status bar itself,
    // so an outer top pad double-insets and leaves a dead strip above the
    // header. Headerless screens (machines/pairing) apply their own top inset.
    <SafeAreaView
      style={{ flex: 1, backgroundColor: colors.bg }}
      edges={["left", "right", "bottom"]}
    >
      {everConnected ? (
        <MainStack onRescan={() => setEverConnected(false)} />
      ) : (
        <MachinesScreen onOpenDiagnostics={() => setShowDiagnostics(true)} />
      )}
      {showDiagnostics ? (
        <View style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: 0, backgroundColor: colors.scrim }}>
          <SafeAreaView style={{ flex: 1 }} edges={["top"]}>
            <DiagnosticsScreen onClose={() => setShowDiagnostics(false)} />
          </SafeAreaView>
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
