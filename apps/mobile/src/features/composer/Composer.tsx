import { useEffect, useState } from "react";
import { View, Text, TextInput, Pressable, ActivityIndicator, Keyboard } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { styles, colors, spacing } from "../theme";

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
            ? Math.max(0, keyboardHeight - insets.bottom) + 16
            : 0,
      }}
    >
      {error ? (
        <Text style={{ color: colors.danger, paddingHorizontal: spacing.sm, paddingTop: spacing.xs }}>
          {error}
        </Text>
      ) : null}
      <View style={{ flexDirection: "row", alignItems: "flex-end", padding: spacing.sm }}>
        <TextInput
          style={[styles.input, { flex: 1, minHeight: 44, maxHeight: 140 }]}
          placeholder="Message the agent…"
          placeholderTextColor={colors.textDim}
          value={text}
          onChangeText={setText}
          multiline
          editable={!disabled}
        />
        <Pressable
          style={[styles.button, { marginLeft: spacing.sm, alignSelf: "stretch", justifyContent: "center" }, !canSend && { opacity: 0.5 }]}
          disabled={!canSend}
          onPress={handleSend}
        >
          {sending ? <ActivityIndicator color="#fff" /> : <Text style={styles.buttonText}>Send</Text>}
        </Pressable>
      </View>
    </View>
  );
}
// end of file
