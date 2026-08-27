import { useEffect, useState } from "react";
import { View, Text, TextInput, Pressable, ActivityIndicator, Keyboard, Platform, Vibration } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { styles, colors, spacing, radius } from "../theme";

interface Props {
  onSend: (text: string) => void | Promise<void>;
  disabled?: boolean;
  error?: string | null;
}

// Prompt input + send. Image/file attach is behind a feature flag for the
// MVP (the prompt content type supports them, but the picker UI is deferred).
//
// Keyboard handling is platform-split to avoid double offsets: iOS relies on
// the KeyboardAvoidingView in ConversationScreen, Android on the default
// adjustResize window mode + a small manual pad here (some keyboards still
// overlap the composer edge with resize mode alone).
export function Composer({ onSend, disabled, error }: Props) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [inputHeight, setInputHeight] = useState(38);
  const [keyboardPad, setKeyboardPad] = useState(0);
  const insets = useSafeAreaInsets();

  useEffect(() => {
    if (Platform.OS === "ios") return;
    const show = Keyboard.addListener("keyboardDidShow", (event) => {
      setKeyboardPad(Math.max(0, event.endCoordinates.height - insets.bottom) + 8);
    });
    const hide = Keyboard.addListener("keyboardDidHide", () => {
      setKeyboardPad(0);
    });
    return () => {
      show.remove();
      hide.remove();
    };
  }, [insets.bottom]);

  const canSend = text.trim().length > 0 && !disabled && !sending;

  const handleSend = async () => {
    if (!canSend) return;
    const value = text.trim();
    setSending(true);
    try {
      await onSend(value);
      Vibration.vibrate(8);
      setText("");
    } catch {
      // The parent surfaces the error and keeps the input so the user can retry.
    } finally {
      setSending(false);
    }
  };

  return (
    <View
      style={{
        backgroundColor: colors.bg,
        borderTopWidth: 1,
        borderTopColor: colors.border,
        paddingBottom: keyboardPad || (insets.bottom > 0 ? 0 : 8),
      }}
    >
      {error ? (
        <View style={{ paddingHorizontal: spacing.md, paddingTop: spacing.xs + 2 }}>
          <View style={[styles.chip, { backgroundColor: colors.dangerTint, borderColor: colors.danger, alignSelf: "flex-start" }]}>
            <Text style={{ color: colors.danger, fontSize: 12 }}>{error}</Text>
          </View>
        </View>
      ) : null}
      <View style={{ flexDirection: "row", alignItems: "flex-end", padding: spacing.sm, gap: spacing.sm }}>
        <TextInput
          style={[
            styles.input,
            {
              flex: 1,
              height: Math.max(46, Math.min(inputHeight + 8, 140)),
              borderRadius: inputHeight <= 50 ? 23 : radius.lg,
              paddingHorizontal: spacing.lg,
              fontSize: 15,
            },
          ]}
          placeholder={"Message the agent\u2026"}
          placeholderTextColor={colors.textFaint}
          value={text}
          onChangeText={setText}
          onContentSizeChange={(event) => setInputHeight(event.nativeEvent.contentSize.height)}
          multiline
          editable={!disabled}
        />
        <Pressable
          style={({ pressed }) => [
            styles.pillButton,
            {
              width: 46,
              height: 46,
              alignSelf: "flex-end",
              opacity: canSend ? (pressed ? 0.85 : 1) : 0.4,
            },
          ]}
          disabled={!canSend}
          onPress={handleSend}
          accessibilityRole="button"
          accessibilityLabel="Send message"
        >
          {sending ? (
            <ActivityIndicator color="#fff" size="small" />
          ) : (
            <Text style={{ color: "#fff", fontSize: 18, fontWeight: "800", lineHeight: 20 }}>{"\u2191"}</Text>
          )}
        </Pressable>
      </View>
    </View>
  );
}
// end of file