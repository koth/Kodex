import { View, Text } from "react-native";
import { colors, spacing, radius } from "../theme";

// Shared empty/error state: a soft glyph medallion + title + hint so blank
// screens read intentional instead of broken.
export function EmptyState({
  glyph = "\u25CB",
  title,
  hint,
}: {
  glyph?: string;
  title: string;
  hint?: string;
}) {
  return (
    <View style={emptyStyles.wrap}>
      <View style={emptyStyles.medallion}>
        <Text style={emptyStyles.glyph}>{glyph}</Text>
      </View>
      <Text style={emptyStyles.title}>{title}</Text>
      {hint ? <Text style={emptyStyles.hint}>{hint}</Text> : null}
    </View>
  );
}

const emptyStyles = {
  wrap: { alignItems: "center" as const, paddingVertical: spacing.xxl, paddingHorizontal: spacing.xl },
  medallion: {
    width: 56,
    height: 56,
    borderRadius: radius.lg,
    alignItems: "center" as const,
    justifyContent: "center" as const,
    backgroundColor: colors.surfaceAlt,
    borderWidth: 1,
    borderColor: colors.border,
    marginBottom: spacing.md,
  },
  glyph: { color: colors.textFaint, fontSize: 22, fontWeight: "600" as const },
  title: { color: colors.textDim, fontSize: 14, fontWeight: "600" as const, textAlign: "center" as const },
  hint: { color: colors.textFaint, fontSize: 12, marginTop: spacing.xs, textAlign: "center" as const, lineHeight: 18 },
};
// end of file