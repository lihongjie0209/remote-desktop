import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ErrorDisplay, { toFriendlyMessage } from "../components/ErrorDisplay";

type ConnState = "connecting" | "connected" | "disconnected" | "error";

interface KeyframePayload {
  width: number;
  height: number;
  data: string; // base64 JPEG — full screen
}

interface TilePayload {
  x: number;
  y: number;
  w: number;
  h: number;
  data: string; // base64 JPEG — tile
}

interface DeltaPayload {
  width: number;
  height: number;
  tiles: TilePayload[];
}

export default function Session() {
  const navigate = useNavigate();
  const location = useLocation();
  const routeRoomId = (location.state as { roomId?: string } | null)?.roomId ?? "";

  const canvasRef = useRef<HTMLCanvasElement>(null);
  // Use a ref so draw callbacks never have stale state and listeners
  // don't need to be re-registered every time connState changes.
  const connStateRef = useRef<ConnState>("connecting");
  const setConnStateRef = (s: ConnState) => {
    connStateRef.current = s;
    setConnState(s);
  };

  const [connState, setConnState] = useState<ConnState>("connecting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // ── Stats (FPS + bandwidth) ────────────────────────────────────────────────
  // Accumulate raw numbers in a ref (no re-render per frame).
  // A 1-second interval reads & resets the accumulator → updates display state.
  const statsAccRef = useRef({ frames: 0, bytes: 0 });
  const [stats, setStats] = useState({ fps: 0, kbps: 0 });

  useEffect(() => {
    const id = setInterval(() => {
      const { frames, bytes } = statsAccRef.current;
      statsAccRef.current = { frames: 0, bytes: 0 };
      setStats({ fps: frames, kbps: Math.round(bytes / 1024) });
    }, 1000);
    return () => clearInterval(id);
  }, []);

  // ── Canvas helpers ─────────────────────────────────────────────────────────

  function ensureSize(canvas: HTMLCanvasElement, w: number, h: number) {
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
  }

  /** Decode a base64 JPEG off the main thread using createImageBitmap. */
  function decodeBitmap(b64: string): Promise<ImageBitmap> {
    // Convert base64 → Uint8Array → Blob → ImageBitmap
    // createImageBitmap uses a browser worker thread, much faster than new Image().
    const binary = atob(b64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return createImageBitmap(new Blob([bytes], { type: "image/jpeg" }));
  }

  // ── Serialized render queue ─────────────────────────────────────────────────
  // At most one frame is queued behind the currently-rendering frame.
  // Excess frames are dropped to keep rendering real-time.
  // This prevents out-of-order rendering (older Promise resolving after newer
  // one) which is the main cause of drag ghosting.
  const renderChain = useRef<Promise<void>>(Promise.resolve());
  const renderPending = useRef(false);

  function enqueueRender(task: () => Promise<void>) {
    if (renderPending.current) {
      // One frame already waiting — drop this frame.
      // The periodic keyframe (every 3 s) will re-sync any stale tiles.
      return;
    }
    renderPending.current = true;
    renderChain.current = renderChain.current
      .then(task)
      .finally(() => { renderPending.current = false; });
  }

  /** Draw a keyframe: decode the full JPEG and paint. Always rendered (never dropped). */
  function drawKeyframe(canvas: HTMLCanvasElement, b64: string) {
    // Keyframes always clear any pending queue and render immediately.
    renderPending.current = false;
    const task = async () => {
      try {
        const bmp = await decodeBitmap(b64);
        const ctx = canvas.getContext("2d");
        if (ctx) { ctx.drawImage(bmp, 0, 0); bmp.close(); }
      } catch { /* ignore */ }
    };
    renderChain.current = renderChain.current.then(task);
  }

  /** Draw a delta frame: decode all tiles in parallel, then paint atomically. */
  function drawDelta(canvas: HTMLCanvasElement, tiles: TilePayload[]) {
    if (tiles.length === 0) return;
    enqueueRender(async () => {
      try {
        const decoded = await Promise.all(
          tiles.map((t) =>
            decodeBitmap(t.data).then((bmp) => ({ bmp, x: t.x, y: t.y }))
          )
        );
        const ctx = canvas.getContext("2d");
        if (ctx) {
          for (const { bmp, x, y } of decoded) {
            ctx.drawImage(bmp, x, y);
            bmp.close();
          }
        }
      } catch { /* ignore */ }
    });
  }

  // ── Tauri event listeners (registered once, use ref for state) ─────────────
  useEffect(() => {
    const unlistens: Array<() => void> = [];

    listen<KeyframePayload>("frame-keyframe", (ev) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const { width, height, data } = ev.payload;
      ensureSize(canvas, width, height);
      drawKeyframe(canvas, data);
      // base64 length × 0.75 ≈ actual JPEG bytes
      statsAccRef.current.frames += 1;
      statsAccRef.current.bytes += Math.round(data.length * 0.75);
      if (connStateRef.current !== "connected") setConnStateRef("connected");
    }).then((fn) => unlistens.push(fn));

    listen<DeltaPayload>("frame-delta", (ev) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const { width, height, tiles } = ev.payload;
      ensureSize(canvas, width, height);
      drawDelta(canvas, tiles);
      statsAccRef.current.frames += 1;
      statsAccRef.current.bytes += tiles.reduce((s, t) => s + Math.round(t.data.length * 0.75), 0);
      if (connStateRef.current !== "connected") setConnStateRef("connected");
    }).then((fn) => unlistens.push(fn));

    listen("peer-disconnected", () => setConnStateRef("disconnected"))
      .then((fn) => unlistens.push(fn));

    listen<string>("connection-error", (ev) => {
      setErrorMsg(ev.payload);
      setConnStateRef("error");
    }).then((fn) => unlistens.push(fn));

    return () => unlistens.forEach((fn) => fn());
  // Register listeners exactly once on mount.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Prevent drag-and-drop ──────────────────────────────────────────────────
  useEffect(() => {
    const prevent = (e: DragEvent) => e.preventDefault();
    document.addEventListener("dragover", prevent);
    document.addEventListener("drop", prevent);
    return () => {
      document.removeEventListener("dragover", prevent);
      document.removeEventListener("drop", prevent);
    };
  }, []);

  // ── Input forwarding ───────────────────────────────────────────────────────
  function normalise(clientX: number, clientY: number): [number, number] {
    const el = canvasRef.current;
    if (!el) return [0, 0];
    const rect = el.getBoundingClientRect();
    return [
      Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)),
      Math.max(0, Math.min(1, (clientY - rect.top) / rect.height)),
    ];
  }

  function mapButton(b: number): string {
    return b === 2 ? "right" : b === 1 ? "middle" : "left";
  }

  useEffect(() => {
    if (connState !== "connected") return;

    const onMouseMove = (e: MouseEvent) => {
      const [x, y] = normalise(e.clientX, e.clientY);
      invoke("send_mouse_move", { x, y }).catch(() => {});
    };
    const onMouseDown = (e: MouseEvent) => {
      e.preventDefault();
      const [x, y] = normalise(e.clientX, e.clientY);
      invoke("send_mouse_move", { x, y }).catch(() => {});
      invoke("send_mouse_button", { button: mapButton(e.button), pressed: true }).catch(() => {});
    };
    const onMouseUp = (e: MouseEvent) => {
      invoke("send_mouse_button", { button: mapButton(e.button), pressed: false }).catch(() => {});
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      invoke("send_mouse_scroll", {
        dx: Math.round(e.deltaX / 40),
        dy: Math.round(e.deltaY / 40),
      }).catch(() => {});
    };
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      invoke("send_key", { key: e.key, pressed: true }).catch(() => {});
    };
    const onKeyUp = (e: KeyboardEvent) => {
      invoke("send_key", { key: e.key, pressed: false }).catch(() => {});
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("wheel", onWheel, { passive: false });
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("wheel", onWheel);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connState]);

  // ── Disconnect ─────────────────────────────────────────────────────────────
  async function disconnect() {
    await invoke("disconnect").catch(() => {});
    navigate("/");
  }

  // ── Render ─────────────────────────────────────────────────────────────────
  return (
    <div className="session-root">
      {/* Canvas wrapper: centres and scales canvas with aspect-ratio preserved */}
      <div className="session-canvas-wrapper">
        <canvas
          ref={canvasRef}
          className="session-video"
          style={{ display: connState === "connected" ? "block" : "none" }}
          onContextMenu={(e) => e.preventDefault()}
        />
      </div>

      {/* Floating toolbar — visible on hover */}
      <div className="session-toolbar">
        <span
          className={`session-status-dot${connState !== "connected" ? " disconnecting" : ""}`}
        />
        {routeRoomId && (
          <span className="session-room-label">{routeRoomId}</span>
        )}
        <span>
          {connState === "connecting" && "正在建立连接…"}
          {connState === "connected" && "已连接"}
          {connState === "disconnected" && "对端已断开"}
          {connState === "error" && (errorMsg ? toFriendlyMessage(errorMsg) : "连接错误")}
        </span>
        {connState === "connected" && (
          <span className="session-stats">
            {stats.fps} fps
            &nbsp;·&nbsp;
            {stats.kbps >= 1024
              ? `${(stats.kbps / 1024).toFixed(1)} MB/s`
              : `${stats.kbps} KB/s`}
          </span>
        )}
        <button className="session-disconnect-btn" onClick={disconnect}>
          断开
        </button>
      </div>

      {connState === "connecting" && (
        <div className="session-loading">
          <div className="session-spinner" />
          <p className="session-loading-text">正在连接…</p>
        </div>
      )}

      {(connState === "error" || connState === "disconnected") && (
        <div className="session-error">
          <div className="session-error-icon">
            {connState === "error" ? "✕" : "⏏"}
          </div>
          {connState === "disconnected" ? (
            <p className="session-error-text">对端已断开连接</p>
          ) : errorMsg ? (
            <ErrorDisplay error={errorMsg} className="session-error-detail" />
          ) : (
            <p className="session-error-text">连接出现错误</p>
          )}
          <button className="btn btn-outline" onClick={disconnect}>
            返回主页
          </button>
        </div>
      )}
    </div>
  );
}