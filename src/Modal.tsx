import { type KeyboardEvent, type ReactNode, useEffect, useRef } from "react";
import { X } from "lucide-react";
import type { Translator } from "./i18n";

export function Modal({
  title,
  children,
  onClose,
  t,
  wide = false,
  dismissible = true,
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  t: Translator;
  wide?: boolean;
  dismissible?: boolean;
}) {
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const previousFocus = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) {
        previousFocus.focus();
      }
    };
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      if (dismissible) onClose();
      return;
    }
    if (event.key !== "Tab") return;

    const controls = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href], summary, [tabindex]:not([tabindex="-1"])',
    ) ?? []).filter((control) => (control.tagName === "SUMMARY" || !control.closest("details:not([open])")) && getComputedStyle(control).visibility !== "hidden");
    if (controls.length === 0) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (document.activeElement === dialogRef.current) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (dismissible && event.currentTarget === event.target) {
          onClose();
        }
      }}
    >
      <section
        aria-labelledby="modal-title"
        aria-modal="true"
        className={"modal-card" + (wide ? " modal-wide" : "")}
        role="dialog"
        tabIndex={-1}
        ref={dialogRef}
        onKeyDown={handleKeyDown}
      >
        <div className="modal-header">
          <h2 id="modal-title">{title}</h2>
          <div className="modal-header-actions">
            <button aria-label={t("common.closeDialog", { title })} className="icon-button" disabled={!dismissible} onClick={onClose} type="button">
              <X size={18} strokeWidth={2} />
            </button>
          </div>
        </div>
        {children}
      </section>
    </div>
  );
}
