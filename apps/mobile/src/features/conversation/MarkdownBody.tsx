import { memo } from "react";
import Markdown from "react-native-markdown-display";
import { colors, spacing, radius } from "../theme";

// Renders assistant/user message bodies as markdown on React Native. The
// desktop uses a custom markdown-it renderer; here we use the RN markdown
// component with a refined dark-first style sheet so output reads premium on
// mobile. Inline code and fences get hairline borders + soft fills.
const markdownStyles: Record<string, object> = {
  body: { color: colors.text, fontSize: 15, lineHeight: 22 },
  paragraph: { marginTop: 0, marginBottom: spacing.sm },
  heading1: { color: colors.text, fontSize: 22, fontWeight: "800", letterSpacing: -0.3, marginTop: spacing.sm, marginBottom: spacing.xs },
  heading2: { color: colors.text, fontSize: 18, fontWeight: "700", marginTop: spacing.sm, marginBottom: spacing.xs },
  heading3: { color: colors.text, fontSize: 16, fontWeight: "700", marginTop: spacing.xs, marginBottom: 2 },
  heading4: { color: colors.text, fontSize: 15, fontWeight: "700" },
  heading5: { color: colors.textDim, fontSize: 14, fontWeight: "700" },
  heading6: { color: colors.textDim, fontSize: 13, fontWeight: "700", textTransform: "uppercase", letterSpacing: 0.6 },
  code_inline: {
    color: colors.accentBright,
    backgroundColor: colors.accentTint,
    fontFamily: "monospace",
    fontSize: 13,
    paddingHorizontal: 5,
    paddingVertical: 1,
    borderRadius: 5,
  },
  fence: {
    color: colors.text,
    backgroundColor: colors.bg,
    fontFamily: "monospace",
    fontSize: 13,
    padding: spacing.md,
    borderRadius: radius.md,
    borderWidth: 1,
    borderColor: colors.border,
    marginTop: spacing.xs,
    marginBottom: spacing.sm,
  },
  bullet_list: { marginVertical: spacing.xs },
  ordered_list: { marginVertical: spacing.xs },
  list_item: { marginVertical: 2 },
  strong: { fontWeight: "800" },
  em: { fontStyle: "italic" },
  link: { color: colors.accent, fontWeight: "600" },
  blockquote: {
    borderLeftWidth: 3,
    borderLeftColor: colors.accent,
    paddingLeft: spacing.md,
    marginVertical: spacing.sm,
    opacity: 0.9,
  },
};

// Memoized on `body`: markdown re-parsing is the most expensive part of a
// timeline row on phones, and it ran for every mounted row on every snapshot
// emit. With memo, only rows whose body actually changed re-parse.
export const MarkdownBody = memo(function MarkdownBody({ body }: { body: string }) {
  return <Markdown style={markdownStyles}>{body}</Markdown>;
});
// end of file