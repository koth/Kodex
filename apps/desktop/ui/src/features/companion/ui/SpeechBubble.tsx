import { useEffect, useState } from "react";
import type { CompanionMood } from "../state/types";

interface SpeechBubbleProps {
  text: string | null;
  mood: CompanionMood;
  onDismiss: () => void;
}

/** 气泡自动消隐时间（ms） */
const AUTO_DISMISS_MS = 8_000;

export function SpeechBubble({ text, mood, onDismiss }: SpeechBubbleProps) {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (!text) {
      setVisible(false);
      return;
    }
    setVisible(true);
    const timer = window.setTimeout(() => {
      setVisible(false);
      onDismiss();
    }, AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [text, onDismiss]);

  if (!text) return null;
  return (
    <div
      className={`companion-bubble companion-bubble-${mood} ${visible ? "is-visible" : ""}`}
      role="status"
      onClick={onDismiss}
    >
      {text}
    </div>
  );
}
