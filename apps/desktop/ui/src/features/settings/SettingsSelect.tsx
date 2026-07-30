import { useEffect, useRef, useState } from "react";

export interface SettingsSelectOption {
  value: string;
  label: string;
}

interface Props {
  ariaLabel: string;
  value: string;
  options: SettingsSelectOption[];
  placeholder?: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}

/**
 * Custom dropdown that replaces native <select>. Native select popups are
 * painted by the OS/WebView and cannot be themed (they stay glaringly light
 * under the dark theme), so settings renders its own listbox instead.
 */
export function SettingsSelect({
  ariaLabel,
  value,
  options,
  placeholder = "— 请选择 —",
  disabled = false,
  onChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const selected = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div
      ref={rootRef}
      className={`settings-provider-select settings-select ${open ? "is-open" : ""}`}
    >
      <button
        type="button"
        className="settings-provider-select-trigger"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        <span className={selected ? "" : "settings-select-placeholder"}>
          {selected ? selected.label : placeholder}
        </span>
        <span className="settings-provider-select-chevron" aria-hidden="true">
          v
        </span>
      </button>
      {open && !disabled && (
        <div className="settings-provider-select-menu" role="listbox" aria-label={ariaLabel}>
          {options.map((option) => {
            const isSelected = option.value === value;
            return (
              <div
                key={option.value}
                className={`settings-provider-select-option ${isSelected ? "is-selected" : ""}`}
                role="option"
                aria-selected={isSelected}
              >
                <button
                  type="button"
                  className="settings-provider-select-option-main"
                  onClick={() => {
                    onChange(option.value);
                    setOpen(false);
                  }}
                >
                  <span>{option.label}</span>
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
