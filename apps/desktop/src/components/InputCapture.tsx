import { RefObject, useCallback, useEffect } from "react";

interface Props {
  /** Ref to the video element — used to compute normalised coordinates. */
  containerRef: RefObject<HTMLVideoElement | null>;
  /**
   * Called with each serialised `DataChannelMessage` JSON object when input
   * is captured.  The caller is responsible for routing it (e.g. over a
   * browser DataChannel or a Tauri invoke).
   */
  onMessage: (msg: Record<string, unknown>) => void;
}

type MouseButton = "left" | "right" | "middle";

function button(b: number): MouseButton {
  return b === 2 ? "right" : b === 1 ? "middle" : "left";
}

/**
 * Transparent overlay that captures mouse, keyboard and clipboard events and
 * forwards them as normalised `DataChannelMessage` JSON objects via the
 * `onMessage` callback.  The parent decides how to route them.
 */
export default function InputCapture({ containerRef, onMessage }: Props) {
  const sendInput = useCallback(
    (msg: Record<string, unknown>) => {
      try {
        onMessage(msg);
      } catch {
        // Silently ignore — connection may be closing.
      }
    },
    [onMessage]
  );

  /** Normalise a pixel position to [0,1] relative to the video element. */
  const normalise = useCallback(
    (clientX: number, clientY: number): [number, number] => {
      const el = containerRef.current;
      if (!el) return [0, 0];
      const rect = el.getBoundingClientRect();
      return [
        Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)),
        Math.max(0, Math.min(1, (clientY - rect.top) / rect.height)),
      ];
    },
    [containerRef]
  );

  // ── Mouse events ──────────────────────────────────────────────────────────

  const onMouseMove = useCallback(
    (e: MouseEvent) => {
      const [x, y] = normalise(e.clientX, e.clientY);
      sendInput({ type: "mouse_move", x, y });
    },
    [normalise, sendInput]
  );

  const onMouseDown = useCallback(
    (e: MouseEvent) => {
      e.preventDefault();
      const [x, y] = normalise(e.clientX, e.clientY);
      sendInput({ type: "mouse_click", button: button(e.button), action: "press", x, y });
    },
    [normalise, sendInput]
  );

  const onMouseUp = useCallback(
    (e: MouseEvent) => {
      const [x, y] = normalise(e.clientX, e.clientY);
      sendInput({ type: "mouse_click", button: button(e.button), action: "release", x, y });
    },
    [normalise, sendInput]
  );

  const onWheel = useCallback(
    (e: WheelEvent) => {
      e.preventDefault();
      sendInput({
        type: "mouse_scroll",
        dx: Math.round(e.deltaX / 40),
        dy: Math.round(e.deltaY / 40),
      });
    },
    [sendInput]
  );

  // ── Keyboard events ───────────────────────────────────────────────────────

  const onKeyDown = useCallback(
    (e: KeyboardEvent) => {
      e.preventDefault();
      sendInput({ type: "key", key: e.key, action: "press" });
    },
    [sendInput]
  );

  const onKeyUp = useCallback(
    (e: KeyboardEvent) => {
      sendInput({ type: "key", key: e.key, action: "release" });
    },
    [sendInput]
  );

  // ── Clipboard paste ───────────────────────────────────────────────────────

  const onPaste = useCallback(
    (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData("text/plain");
      if (text) sendInput({ type: "clipboard", text });
    },
    [sendInput]
  );

  // ── Register / unregister listeners ───────────────────────────────────────

  useEffect(() => {
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("wheel", onWheel, { passive: false });
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("paste", onPaste);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("wheel", onWheel);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("paste", onPaste);
    };
  }, [onMouseMove, onMouseDown, onMouseUp, onWheel, onKeyDown, onKeyUp, onPaste]);

  // Invisible full-screen overlay to eat right-click context menus.
  return (
    <div
      onContextMenu={(e) => e.preventDefault()}
      style={{
        position: "absolute",
        inset: 0,
        zIndex: 5,
        cursor: "none",
      }}
    />
  );
}
