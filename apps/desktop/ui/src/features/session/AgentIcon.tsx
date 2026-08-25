/**
 * Brand marks for the agents a session can run with. Rendered as inline SVG so
 * they inherit sizing from the sidebar row and need no asset pipeline.
 *
 * Only three agents get a brand mark (codex / claude / deepseek); anything else
 * falls back to a neutral glyph so rows stay aligned.
 */

export type AgentIconKind = "codex" | "claude" | "deepseek" | "unknown";

export function resolveAgentKind(agentCli?: string | null): AgentIconKind {
  // Session rows store the raw `agent_cli` string (e.g. "codex-acp",
  // "deepseek-harness"); older rows may carry a display label like "Codex".
  const raw = agentCli?.trim().toLowerCase() ?? "";
  if (raw.includes("deepseek")) return "deepseek";
  if (raw.includes("claude")) return "claude";
  if (raw.includes("codex")) return "codex";
  return "unknown";
}

interface AgentIconProps {
  agentCli?: string | null;
  className?: string;
}

export function AgentIcon({ agentCli, className }: AgentIconProps) {
  switch (resolveAgentKind(agentCli)) {
    case "codex":
      return <CodexMark className={className} />;
    case "claude":
      return <ClaudeMark className={className} />;
    case "deepseek":
      return <DeepSeekMark className={className} />;
    default:
      return <UnknownAgentMark className={className} />;
  }
}

/** OpenAI knot — Codex sessions follow the text color so it reads on both themes. */
function CodexMark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.8956zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997z" />
    </svg>
  );
}

/** Anthropic Claude starburst in the brand coral. */
function ClaudeMark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="#D97757" strokeWidth="2.9" strokeLinecap="round" aria-hidden="true">
      {/* Long vertical/horizontal rays */}
      <path d="M12 2.6v18.8M2.6 12h18.8" />
      {/* Diagonal rays, slightly shorter like the real mark */}
      <path d="M5.75 5.75l12.5 12.5M18.25 5.75l-12.5 12.5" />
    </svg>
  );
}

/** DeepSeek whale swoosh in the brand blue. */
function DeepSeekMark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="#4D6BFE" aria-hidden="true">
      <path d="M23.748 4.482c-.254-.124-.364-.072-.512.03-.208.148-.416.301-.622.455-.838.63-1.758 1.055-2.803 1.113-1.978.112-3.105-.87-3.105-.87s-.892 1.32-2.738 2.756c-1.94 1.51-4.842 3.177-4.842 3.177.897-2.124 1.42-5.245 1.42-5.245s-.65 1.941-2.005 3.666c-1.355 1.725-3.176 2.754-3.176 2.754S3.3 11.24 2.6 8.595C2.19 7.062 2.32 5.47 2.32 5.47s-1.32 1.94-.838 5.245c.483 3.304 2.754 5.245 2.754 5.245s-.09 1.941.483 3.666c.572 1.725 1.94 2.926 1.94 2.926s-.207-1.51.483-2.926c.69-1.415 1.94-2.321 1.94-2.321s2.004.207 4.086-.208c2.083-.415 3.851-1.533 3.851-1.533s2.927.09 5.246-1.42c2.319-1.51 2.881-4.086 2.881-4.086s-.87.87-2.004 1.356c-1.134.483-2.321.543-2.321.543s1.94-1.32 2.753-2.754c.813-1.434.156-3.218.156-3.218Z" />
    </svg>
  );
}

/** Neutral fallback so legacy sessions without an agent keep row alignment. */
function UnknownAgentMark({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" opacity="0.55" aria-hidden="true">
      <rect x="4.5" y="8" width="15" height="11" rx="3" />
      <path d="M12 8V4.8M12 4.8a1.4 1.4 0 1 0 0-2.8 1.4 1.4 0 0 0 0 2.8Z" />
      <path d="M9 12.6v1.6M15 12.6v1.6" />
    </svg>
  );
}
