import { type ReactNode, useEffect, useId, useRef, useState } from "react";

export function PopoverMenu({
  className,
  label,
  title,
  popoverLabel,
  icon,
  children,
}: {
  className: string;
  label: string;
  title?: string;
  popoverLabel?: string;
  icon: ReactNode;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverId = useId();

  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    const closeOnOutsidePress = (event: PointerEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) close();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      close();
      triggerRef.current?.focus();
    };
    document.addEventListener("pointerdown", closeOnOutsidePress);
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePress);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", close);
    };
  }, [open]);

  return (
    <div
      className={className}
      ref={rootRef}
      onBlur={(event) => {
        // WebKit does not focus a clicked button, so a missing target is not
        // proof that focus left the menu; the pointer listener covers that.
        if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
    >
      <button
        aria-controls={open ? popoverId : undefined}
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen((current) => !current)}
        ref={triggerRef}
        title={title}
        type="button"
      >
        {icon}
      </button>
      {open ? (
        <div
          aria-label={popoverLabel}
          className={className + "-popover"}
          id={popoverId}
          onClick={(event) => {
            if (!(event.target instanceof Element) || !event.target.closest("button")) return;
            // Return focus first so a dialog opened by this action restores it
            // to the trigger rather than to a removed menu item.
            triggerRef.current?.focus();
            setOpen(false);
          }}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
}
