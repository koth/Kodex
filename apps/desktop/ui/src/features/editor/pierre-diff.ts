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
      --review-diff-scrollbar-thumb: color-mix(in srgb, var(--app-bg) 78%, var(--text-soft)) !important;
      --review-diff-scrollbar-thumb-active: color-mix(in srgb, var(--app-bg) 60%, var(--text-soft)) !important;
      --review-diff-scrollbar-thumb-hover: color-mix(in srgb, var(--app-bg) 45%, var(--text-soft)) !important;
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

    // The pierre renderer puts its own scrollbars INSIDE the shadow DOM on
    // [data-code] (overflow: scroll). The vertical one is the bright bar that
    // appears once content is long enough to scroll; its thumb falls back to
    // the library's default light --diffs-bg-context. Recolor it to a theme
    // color that stays close to the app background. Keep overflow-y scrolling
    // intact — pierre's [data-code] is the vertical scroll container, so hiding
    // the scrollbar or clipping overflow would break scrolling for long diffs.
    // Note: the thumb uses background-clip: content-box with a transparent
    // border, so use an opaque surface color (a translucent one reads as a
    // washed-out bright stripe).
    [data-code],
    [data-overflow="scroll"] [data-code] {
      scrollbar-color: var(--review-diff-scrollbar-thumb) transparent !important;
    }

    :host(:hover) [data-code],
    :host(:hover) [data-overflow="scroll"] [data-code],
    :is([data-diff], [data-file]):hover [data-code] {
      scrollbar-color: var(--review-diff-scrollbar-thumb-active) transparent !important;
    }

    [data-code]::-webkit-scrollbar-thumb,
    [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb {
      background-color: var(--review-diff-scrollbar-thumb) !important;
      background-clip: content-box !important;
    }

    :host(:hover) [data-code]::-webkit-scrollbar-thumb,
    :host(:hover) [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb,
    :is([data-diff], [data-file]):hover [data-code]::-webkit-scrollbar-thumb {
      background-color: var(--review-diff-scrollbar-thumb-active) !important;
      background-clip: content-box !important;
    }

    [data-code]::-webkit-scrollbar-thumb:hover,
    [data-overflow="scroll"] [data-code]::-webkit-scrollbar-thumb:hover {
      background-color: var(--review-diff-scrollbar-thumb-hover) !important;
    }

    // The <pre> element is pierre's actual vertical scroll container (it gets
    // tabindex="0" and overflow:scroll from the library, but the library gives
    // it NO scrollbar styling at all). Its vertical scrollbar therefore renders
    // with the WebKit default light track/thumb — the bright bar that shows up
    // once content is long enough. Theme it here so it matches the app surface.
    pre,
    [data-diff],
    [data-file] {
      scrollbar-color: var(--review-diff-scrollbar-thumb) transparent !important;
      scrollbar-width: thin !important;
    }

    pre::-webkit-scrollbar,
    [data-diff]::-webkit-scrollbar,
    [data-file]::-webkit-scrollbar {
      width: 9px !important;
      height: 9px !important;
    }

    pre::-webkit-scrollbar-track,
    [data-diff]::-webkit-scrollbar-track,
    [data-file]::-webkit-scrollbar-track {
      background: transparent !important;
    }

    pre::-webkit-scrollbar-thumb,
    [data-diff]::-webkit-scrollbar-thumb,
    [data-file]::-webkit-scrollbar-thumb {
      border: 2px solid transparent !important;
      border-radius: 999px !important;
      background-color: var(--review-diff-scrollbar-thumb) !important;
      background-clip: content-box !important;
    }

    pre:hover::-webkit-scrollbar-thumb,
    [data-diff]:hover::-webkit-scrollbar-thumb,
    [data-file]:hover::-webkit-scrollbar-thumb {
      background-color: var(--review-diff-scrollbar-thumb-active) !important;
    }

    pre::-webkit-scrollbar-thumb:hover,
    [data-diff]::-webkit-scrollbar-thumb:hover,
    [data-file]::-webkit-scrollbar-thumb:hover {
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
