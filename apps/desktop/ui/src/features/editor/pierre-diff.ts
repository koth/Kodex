import type { FileContents } from "@pierre/diffs/react";
import type { AppTheme, DiffQuality, FileChangeRecord, SessionFileChange } from "../../types";

export const PIERRE_DIFF_SCROLL_TARGET_SELECTOR =
  "[data-code], pre, [data-diff], [data-content]";

export const PIERRE_DIFF_OPTIONS_BASE = {
  disableFileHeader: true,
  hunkSeparators: "line-info",
  collapsedContextThreshold: 6,
  expansionLineCount: 80,
  lineDiffType: "none",
  overflow: "scroll",
  unsafeCSS: `
    :host {
      --diffs-bg: var(--app-bg) !important;
      --diffs-dark-bg: var(--app-bg) !important;
      --diffs-light-bg: var(--app-bg) !important;
      --diffs-bg-context: var(--app-bg) !important;
      --diffs-bg-buffer: var(--app-bg) !important;
      --review-diff-scrollbar-thumb: color-mix(in srgb, var(--app-bg) 82%, var(--text-soft)) !important;
      --review-diff-scrollbar-thumb-active: color-mix(in srgb, var(--app-bg) 56%, var(--text-muted)) !important;
      --review-diff-scrollbar-thumb-hover: color-mix(in srgb, var(--app-bg) 34%, var(--text-muted)) !important;
      background-color: var(--app-bg) !important;
    }

    pre,
    code,
    [data-code],
    [data-diff],
    [data-file],
    [data-gutter],
    [data-content] {
      background-color: var(--diffs-bg) !important;
    }

    :where([data-background]) [data-line-type="context"],
    :where([data-background]) [data-line-type="context-expanded"],
    :where([data-background]) [data-gutter-buffer],
    :where([data-background]) [data-column-number]:not([data-line-type="change-addition"]):not([data-line-type="change-deletion"]),
    :where([data-background]) [data-line]:not([data-line-type="change-addition"]):not([data-line-type="change-deletion"]),
    :where([data-background]) [data-no-newline]:not([data-line-type="change-addition"]):not([data-line-type="change-deletion"]) {
      --diffs-computed-decoration-bg: var(--diffs-bg) !important;
      --diffs-computed-diff-line-bg: var(--diffs-bg) !important;
      --diffs-computed-selected-line-bg: var(--diffs-bg) !important;
      --diffs-line-bg: var(--diffs-bg) !important;
      background-color: var(--diffs-bg) !important;
    }

    [data-line-type="context"],
    [data-line-type="context-expanded"],
    [data-line-annotation],
    [data-gutter-buffer="annotation"] {
      --diffs-line-bg: var(--diffs-bg) !important;
      background-color: var(--diffs-bg) !important;
    }

    [data-content-buffer],
    [data-gutter-buffer="buffer"] {
      --diffs-line-bg: var(--diffs-bg) !important;
      background-color: var(--diffs-bg) !important;
      background-image: none !important;
    }

    [data-overflow="scroll"] [data-code] {
      overflow-x: auto !important;
      overflow-y: clip !important;
      scrollbar-color: var(--review-diff-scrollbar-thumb) transparent !important;
      scrollbar-width: thin !important;
    }

    :host(:hover) [data-overflow="scroll"] [data-code] {
      scrollbar-color: var(--review-diff-scrollbar-thumb-active) transparent !important;
    }

    [data-overflow="scroll"] [data-code]::-webkit-scrollbar {
      width: 0 !important;
      height: 9px !important;
    }

    [data-overflow="scroll"] [data-code]::-webkit-scrollbar-track {
      background: transparent !important;
    }

    [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb {
      min-width: 36px !important;
      border: 2px solid transparent !important;
      border-radius: 999px !important;
      background-color: var(--review-diff-scrollbar-thumb) !important;
      background-clip: content-box !important;
    }

    :host(:hover) [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb {
      background-color: var(--review-diff-scrollbar-thumb-active) !important;
    }

    [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb:hover {
      background-color: var(--review-diff-scrollbar-thumb-hover) !important;
    }
  `,
} as const;

export type PierreDiffPreview =
  | { kind: "patch"; oldFile: FileContents; newFile: FileContents }
  | { kind: "message"; text: string };

type PierreDiffChange = Pick<
  FileChangeRecord | SessionFileChange,
  "path" | "added_lines" | "removed_lines"
> & {
  old_text?: string | null;
  new_text?: string | null;
  quality?: DiffQuality;
  updated_at?: string | null;
  timestamp?: string | null;
};

export function pierreDiffOptions(appTheme: AppTheme, diffStyle: "unified" | "split" = "unified") {
  return {
    ...PIERRE_DIFF_OPTIONS_BASE,
    diffStyle,
    theme: appTheme === "light" ? "pierre-light" : "pierre-dark",
    themeType: appTheme === "light" ? "light" : "dark",
  } as const;
}

export function diffQualityMessage(quality: DiffQuality) {
  const labels: Record<DiffQuality, string | null> = {
    Exact: null,
    LargeFileSkipped: "文件太大，已跳过内联差异预览。",
    BinarySkipped: "二进制或不可读取文件，无法展示文本差异。",
    MissingBaseline: "缺少可比较的基线内容，无法展示可靠差异。",
    FragmentRejected: "只捕获到了片段级改动，已拒绝渲染为完整文件差异。",
    LegacyIncomplete: "旧历史记录缺少完整快照，无法展示可靠差异。",
  };
  return labels[quality] ?? null;
}

export function buildPierreDiff(change: PierreDiffChange): PierreDiffPreview {
  const quality = change.quality ?? "Exact";
  const unavailable = diffQualityMessage(quality);
  if (unavailable) {
    return { kind: "message", text: unavailable };
  }
  if (!("old_text" in change) || !("new_text" in change)) {
    return { kind: "message", text: "正在加载差异..." };
  }

  const oldText = change.old_text ?? "";
  const newText = change.new_text ?? "";
  if (oldText === newText) {
    return { kind: "message", text: "暂无可预览的文本差异" };
  }

  const updatedAt = change.updated_at ?? change.timestamp ?? "";
  return {
    kind: "patch",
    oldFile: {
      name: change.path,
      contents: oldText,
      cacheKey: `${change.path}:old:${oldText.length}:${updatedAt}`,
    },
    newFile: {
      name: change.path,
      contents: newText,
      cacheKey: `${change.path}:new:${newText.length}:${updatedAt}`,
    },
  };
}

export function resolvePierreDiffHorizontalScrollTarget(root: HTMLDivElement) {
  const candidates = collectPierreDiffScrollTargets(root).filter(isHorizontallyScrollable);
  if (candidates.length === 0) return root;

  const activeElement = typeof document !== "undefined" ? document.activeElement : null;
  if (activeElement) {
    const activeCandidate = candidates.find((candidate) =>
      candidateContainsActiveElement(candidate, activeElement),
    );
    if (activeCandidate) return activeCandidate;
  }

  const hoveredCandidate = candidates.find(isHoveredElement);
  return hoveredCandidate ?? candidates[0] ?? root;
}

function collectPierreDiffScrollTargets(root: HTMLDivElement) {
  const targets: HTMLElement[] = [];
  const seenTargets = new Set<HTMLElement>();
  const seenScopes = new Set<Document | DocumentFragment | Element>();

  const addTarget = (target: HTMLElement) => {
    if (seenTargets.has(target)) return;
    seenTargets.add(target);
    targets.push(target);
  };

  const collectFromScope = (scope: Document | DocumentFragment | Element) => {
    if (seenScopes.has(scope)) return;
    seenScopes.add(scope);

    if (
      scope instanceof HTMLElement &&
      scope.matches(PIERRE_DIFF_SCROLL_TARGET_SELECTOR)
    ) {
      addTarget(scope);
    }

    for (const element of Array.from(
      scope.querySelectorAll<HTMLElement>(PIERRE_DIFF_SCROLL_TARGET_SELECTOR),
    )) {
      addTarget(element);
    }

    for (const element of Array.from(scope.querySelectorAll<HTMLElement>("*"))) {
      if (element.shadowRoot) collectFromScope(element.shadowRoot);
    }
  };

  collectFromScope(root);
  addTarget(root);
  return targets;
}

function isHorizontallyScrollable(element: HTMLElement) {
  return element.scrollWidth > element.clientWidth + 1;
}

function isHoveredElement(element: HTMLElement) {
  try {
    return element.matches(":hover");
  } catch {
    return false;
  }
}

function candidateContainsActiveElement(candidate: HTMLElement, activeElement: Element) {
  if (candidate === activeElement || candidate.contains(activeElement)) return true;

  const candidateRoot = candidate.getRootNode();
  return Boolean(
    typeof ShadowRoot !== "undefined" &&
      candidateRoot instanceof ShadowRoot &&
      candidateRoot.host === activeElement,
  );
}
