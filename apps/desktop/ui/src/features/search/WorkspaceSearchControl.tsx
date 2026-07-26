import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { SearchResult } from "../../types";
import { fsSearch } from "../../lib/tauri";
import {
  openSearchSelectableItem,
  SearchResults,
} from "./SearchResults";
import "./WorkspaceSearchControl.css";

interface Props {
  onFileOpen: (filePath: string, lineNumber?: number, searchQuery?: string) => void;
  className?: string;
  buttonClassName?: string;
}

export function WorkspaceSearchControl({
  onFileOpen,
  className,
  buttonClassName,
}: Props) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResult, setSearchResult] = useState<SearchResult | null>(null);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [activeCount, setActiveCount] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const searchRequestRef = useRef(0);

  const closeSearch = useCallback(() => {
    searchRequestRef.current += 1;
    setSearchOpen(false);
    setSearchResult(null);
    setSearchError(null);
    setSearchLoading(false);
    setActiveIndex(-1);
    setActiveCount(0);
  }, []);

  const toggleSearch = useCallback(() => {
    if (searchOpen) {
      closeSearch();
    } else {
      setSearchOpen(true);
    }
  }, [searchOpen, closeSearch]);

  useEffect(() => {
    if (searchOpen && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [searchOpen]);

  useEffect(() => {
    if (!searchOpen) return;
    const handleClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (popoverRef.current && popoverRef.current.contains(target)) return;
      const trigger = document.querySelector(".workspace-search-trigger");
      if (trigger && trigger.contains(target)) return;
      closeSearch();
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeSearch();
      }
    };
    document.addEventListener("mousedown", handleClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [searchOpen, closeSearch]);

  const runSearch = useCallback(async (q: string) => {
    const requestId = ++searchRequestRef.current;
    setSearchLoading(true);
    setSearchError(null);
    setSearchResult(null);
    setActiveIndex(-1);
    try {
      const result = await fsSearch(q);
      if (searchRequestRef.current === requestId) {
        setSearchResult(result);
      }
    } catch (err) {
      if (searchRequestRef.current === requestId) {
        setSearchError(String(err));
      }
    } finally {
      if (searchRequestRef.current === requestId) {
        setSearchLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    if (!searchOpen) return;
    const q = searchQuery.trim();
    if (!q) {
      searchRequestRef.current += 1;
      setSearchResult(null);
      setSearchError(null);
      setSearchLoading(false);
      setActiveIndex(-1);
      setActiveCount(0);
      return;
    }
    const timer = window.setTimeout(() => {
      void runSearch(q);
    }, 220);
    return () => window.clearTimeout(timer);
  }, [runSearch, searchOpen, searchQuery]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (activeCount <= 0) return;
        setActiveIndex((prev) => (prev + 1 + activeCount) % activeCount);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (activeCount <= 0) return;
        setActiveIndex((prev) => (prev <= 0 ? activeCount - 1 : prev - 1));
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        if (searchResult && activeIndex >= 0) {
          if (openSearchSelectableItem(searchResult, activeIndex, onFileOpen)) {
            closeSearch();
          }
          return;
        }
        const q = searchQuery.trim();
        if (q) void runSearch(q);
        return;
      } else if (e.key === "Escape") {
        closeSearch();
      }
    },
    [
      activeCount,
      activeIndex,
      closeSearch,
      onFileOpen,
      runSearch,
      searchQuery,
      searchResult,
    ],
  );

  // Keep the results pane mounted while open so empty/hint states stay visible.
  const showResults = searchOpen;
  const searchTitle = "搜索工作区";
  const triggerClass = [
    "thread-header-action-btn",
    "workspace-search-trigger",
    buttonClassName ?? "",
    searchOpen ? "is-active" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <>
      <button
        type="button"
        className={triggerClass}
        onClick={toggleSearch}
        title={searchTitle}
        aria-label={searchTitle}
        aria-expanded={searchOpen}
        aria-haspopup="dialog"
      >
        <SearchIcon />
      </button>
      {searchOpen &&
        createPortal(
          <div className="workspace-search-overlay" role="presentation">
            <div
              className="workspace-search-popover"
              ref={popoverRef}
              role="dialog"
              aria-modal="true"
              aria-label={searchTitle}
            >
              <div className="workspace-search-bar">
                <span className="workspace-search-bar-icon" aria-hidden="true">
                  <SearchIcon />
                </span>
                <input
                  ref={inputRef}
                  type="text"
                  className="workspace-search-input"
                  placeholder="搜索文件名或文件内容..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  onKeyDown={handleKeyDown}
                  spellCheck={false}
                  autoComplete="off"
                />
                {searchQuery && (
                  <button
                    type="button"
                    className="workspace-search-clear"
                    onClick={() => {
                      setSearchQuery("");
                      setSearchResult(null);
                      setSearchError(null);
                      inputRef.current?.focus();
                    }}
                    aria-label="清空搜索"
                    title="清空"
                  >
                    ×
                  </button>
                )}
                <kbd className="workspace-search-esc">esc</kbd>
              </div>
              {showResults && (
                <SearchResults
                  result={searchResult}
                  loading={searchLoading}
                  error={searchError}
                  query={searchQuery}
                  onFileOpen={onFileOpen}
                  onClose={closeSearch}
                  placement="inline"
                  activeIndex={activeIndex}
                  onActiveIndexChange={setActiveIndex}
                  onActiveCountChange={setActiveCount}
                />
              )}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}

function SearchIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="11" cy="11" r="7" />
      <path d="m16 16 5 5" />
    </svg>
  );
}
