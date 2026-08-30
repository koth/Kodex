// Port of the desktop `splitUserMessageBody` (apps/desktop/ui
// ConversationTimeline): user prompts embed attached images as self-contained
// markdown image blocks (`![alt](data:image/...;base64,...)`). Rendering that
// body through the markdown component shows the raw base64 (the RN markdown
// parser does not reliably handle megabyte data URLs), so image markdown is
// split out and rendered as native thumbnails instead — mirroring the desktop
// strip.

export interface UserMessageImage {
  alt: string;
  src: string;
}

// Global sweep instead of whole-block matching: the PC composes
// `text\n\n![Image: alt](data:...;base64,... "file://...")`, but coalesced
// patch merges or steering bodies may end up single-newline separated or with
// stray whitespace — a positional parse would silently leave megabyte base64
// in the rendered text. The PC may also append a quoted original-file title
// (`"file://..."`) after the URL — tolerated and ignored (the phone has no
// filesystem access to it).
const IMAGE_MARKDOWN_RE =
  /!\[([^\]]*)\]\((data:image\/(?:apng|avif|bmp|png|jpeg|jpg|gif|webp);base64,[A-Za-z0-9+/=]+)(?:\s+"[^"]*")?\)/gi;

export function splitUserMessageBody(body: string): { text: string; images: UserMessageImage[] } {
  const images: UserMessageImage[] = [];
  const text = body
    .replace(IMAGE_MARKDOWN_RE, (_match: string, alt: string, src: string) => {
      images.push({ alt: alt.trim(), src });
      return "";
    })
    // Collapse the whitespace left behind where images were pulled out.
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  return { text, images };
}
