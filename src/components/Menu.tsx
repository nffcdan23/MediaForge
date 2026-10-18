// Context menus and the log popover, drawn by the app so they look the same on both platforms.

import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";

export type MenuItem = { label: string; icon?: ReactNode; onSelect: () => void; destructive?: boolean } | "separator";

/** Opens a context menu at the pointer. Returns the handler to attach and the menu to render. */
export function useContextMenu(items: () => MenuItem[]) {
  const [position, setPosition] = useState<{ x: number; y: number } | null>(null);
  const open = (event: ReactMouseEvent) => {
    event.preventDefault();
    setPosition({ x: event.clientX, y: event.clientY });
  };
  const menu = position ? <ContextMenu x={position.x} y={position.y} items={items()} onClose={() => setPosition(null)} /> : null;
  return { onContextMenu: open, menu };
}

function ContextMenu({ x, y, items, onClose }: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const rect = ref.current?.getBoundingClientRect();
    if (!rect) return;
    setPlace({
      left: Math.min(x, window.innerWidth - rect.width - 8),
      top: Math.min(y, window.innerHeight - rect.height - 8),
    });
  }, [x, y]);

  useEffect(() => {
    const close = (event: Event) => {
      if (event instanceof MouseEvent && ref.current?.contains(event.target as Node)) return;
      onClose();
    };
    const escape = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("mousedown", close, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("keydown", escape);
    document.addEventListener("scroll", onClose, true);
    return () => {
      window.removeEventListener("mousedown", close, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("keydown", escape);
      document.removeEventListener("scroll", onClose, true);
    };
  }, [onClose]);

  return (
    <div ref={ref} className="menu" style={place} role="menu">
      {items.map((item, index) =>
        item === "separator" ? (
          <hr key={index} />
        ) : (
          <button
            key={index}
            type="button"
            role="menuitem"
            className={item.destructive ? "destructive" : ""}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            {item.icon}
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}

/** Removes separators that would appear first, last or twice in a row. */
export function tidy(items: (MenuItem | false | null | undefined)[]): MenuItem[] {
  const present = items.filter((i): i is MenuItem => !!i);
  return present.filter((item, index) => {
    if (item !== "separator") return true;
    const previous = present.slice(0, index).filter((i) => i !== undefined);
    return previous.length > 0 && previous[previous.length - 1] !== "separator" && index < present.length - 1;
  });
}
