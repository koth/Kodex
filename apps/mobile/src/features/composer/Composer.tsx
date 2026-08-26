import { useEffect, useState } from "react";
import { View, Text, TextInput, Pressable, ActivityIndicator, Keyboard } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { styles, colors, spacing, radius } from "../theme";

interface Props {
  onSend: (text: string) => void | Promise<void>;
  disabled?: boolean;
  error?: string | null;
}

// Prompt input + send. Image/file attach is behind a feature flag for the
// MVP (the prompt content type supports them, but the picker UI is deferred).
export function Composer({ onSend, disabled, error }: Props) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [keyboardHeight, setKeyboardHeight] = useState(0);
  const insets = useSafeAreaInsets();

  useEffect(() => {
    const show = Keyboard.addListener("keyboardDidShow", (event) => {
      setKeyboardHeight(event.endCoordinates.height);
    });
    const hide = Keyboard.addListener("keyboardDidHide", () => {
      setKeyboardHeight(0);
    });
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  const canSend = text.trim().length > 0 && !disabled && !sending;

  const handleSend = async () => {
    if (!canSend) return;
    const value = text.trim();
    setSending(true);
    try {
      await onSend(value);
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
        paddingBottom:
          keyboardHeight > 0
            ? Math.max(0, keyboardHeight - insets.bottom) + 12
            : insets.bottom > 0
              ? 0
              : 8,
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
              minHeight: 46,
              maxHeight: 140,
              borderRadius: radius.pill,
              paddingHorizontal: spacing.lg,
              fontSize: 15,
            },
          ]}
          placeholder={"Message the agent\u2026"}
          placeholderTextColor={colors.textFaint}
          value={text}
          onChangeText={setText}
          multiline
          editable={!disabled}
        />
        <Pressable
          style={({ pressed }) => [
            styles.pillButton,
            {
              width: 46,
              minHeight: 46,
              height: "100%",
              opacity: canSend ? (pressed ? 0.85 : 1) : 0.4,
            },
          ]}
          disabled={!canSend}
          onPress={handleSend}
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