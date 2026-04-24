import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type HostStatus = "idle" | "starting" | "waiting" | "hosting" | "error";
type ClientStatus = "idle" | "connecting" | "error";

const STORAGE_PWD = "rdsk_pwd";
const STORAGE_SERVER_URL = "rdsk_server_url";
const STORAGE_HISTORY = "rdsk_history";
const MAX_HISTORY = 10;

interface HistoryEntry {
  deviceId: string;
  password: string;
  lastConnected: number; // unix ms
}

function loadHistory(): HistoryEntry[] {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_HISTORY) ?? "[]");
  } catch {
    return [];
  }
}

function saveHistory(entries: HistoryEntry[]) {
  localStorage.setItem(STORAGE_HISTORY, JSON.stringify(entries));
}

function addHistory(deviceId: string, password: string): HistoryEntry[] {
  const entries = loadHistory().filter((e) => e.deviceId !== deviceId);
  const updated = [
    { deviceId, password, lastConnected: Date.now() },
    ...entries,
  ].slice(0, MAX_HISTORY);
  saveHistory(updated);
  return updated;
}

function removeHistory(deviceId: string): HistoryEntry[] {
  const updated = loadHistory().filter((e) => e.deviceId !== deviceId);
  saveHistory(updated);
  return updated;
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  const m = Math.floor(diff / 60_000);
  const h = Math.floor(diff / 3_600_000);
  const d = Math.floor(diff / 86_400_000);
  if (d > 0) return `${d}天前`;
  if (h > 0) return `${h}小时前`;
  if (m > 0) return `${m}分钟前`;
  return "刚刚";
}

function genPassword(len = 6): string {
  return Array.from({ length: len }, () => Math.floor(Math.random() * 10)).join("");
}

export default function Remote() {
  const navigate = useNavigate();
  const autoStarted = useRef(false);

  const [hostPwd, setHostPwd] = useState(
    () => localStorage.getItem(STORAGE_PWD) ?? genPassword(6)
  );
  const [pwdDraft, setPwdDraft] = useState<string | null>(null);
  const [showHostPwd, setShowHostPwd] = useState(false);
  const [hostStatus, setHostStatus] = useState<HostStatus>("idle");
  const [roomId, setRoomId] = useState<string | null>(null);
  const [hostError, setHostError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const [partnerId, setPartnerId] = useState("");
  const [clientPwd, setClientPwd] = useState("");
  const [clientStatus, setClientStatus] = useState<ClientStatus>("idle");
  const [clientError, setClientError] = useState<string | null>(null);

  const [history, setHistory] = useState<HistoryEntry[]>(loadHistory);

  const serverUrl = (localStorage.getItem(STORAGE_SERVER_URL) ?? "https://remote-desktop.xuntukeji.cn")
    .replace(/localhost/g, "127.0.0.1");

  useEffect(() => {
    const unlistens: UnlistenFn[] = [];
    listen<string>("peer-connected", () => setHostStatus("hosting")).then((fn) => unlistens.push(fn));
    listen<string>("connection-error", (ev) => { setHostError(ev.payload); setHostStatus("error"); }).then((fn) => unlistens.push(fn));
    listen("peer-disconnected", () => { setHostStatus("waiting"); setHostError(null); }).then((fn) => unlistens.push(fn));
    return () => unlistens.forEach((fn) => fn());
  }, []);

  const startHosting = useCallback(async (pwd: string, url: string, did: string | null) => {
    if (!pwd) return;
    setHostStatus("starting");
    setHostError(null);

    const MAX_RETRIES = 5;
    const RETRY_DELAYS = [1000, 2000, 3000, 4000, 5000]; // ms

    for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
      try {
        await invoke("set_server_url", { url });
        const id = await invoke<string>("create_room", { password: pwd, deviceId: did });
        localStorage.setItem(STORAGE_PWD, pwd);
        setRoomId(id);
        setHostStatus("waiting");
        setHostError(null);
        return;
      } catch (e) {
        if (attempt < MAX_RETRIES) {
          const delay = RETRY_DELAYS[attempt];
          setHostError(`连接失败，${delay / 1000}s 后重试 (${attempt + 1}/${MAX_RETRIES})…`);
          await new Promise((r) => setTimeout(r, delay));
          // still "starting" while retrying
        } else {
          setHostError(String(e));
          setHostStatus("error");
        }
      }
    }
  }, []);

  useEffect(() => {
    if (autoStarted.current) return;
    autoStarted.current = true;
    invoke<string>("get_or_create_device_id")
      .then((did) => startHosting(hostPwd, serverUrl, did))
      .catch(() => startHosting(hostPwd, serverUrl, null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function stopHosting() {
    await invoke("disconnect").catch(() => {});
    setHostStatus("idle");
    setRoomId(null);
    setHostError(null);
  }

  async function copyId() {
    if (!roomId) return;
    await navigator.clipboard.writeText(roomId);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  }

  function formatId(id: string): string {
    if (id.length === 8) return `${id.slice(0, 4)} · ${id.slice(4)}`;
    return id.match(/.{1,4}/g)?.join(" · ") ?? id;
  }

  const connectToPartner = useCallback(async (targetId?: string, targetPwd?: string) => {
    const id = (targetId ?? partnerId).trim().toUpperCase();
    const pwd = targetPwd !== undefined ? targetPwd : clientPwd;
    if (!id) return;
    setPartnerId(id);
    setClientPwd(pwd);
    setClientStatus("connecting");
    setClientError(null);
    try {
      await invoke("set_server_url", { url: serverUrl });
      await invoke("join_room", { roomId: id, password: pwd });
      setHistory(addHistory(id, pwd));
      navigate("/session", { state: { roomId: id } });
    } catch (e) {
      setClientError(String(e));
      setClientStatus("error");
    }
  }, [partnerId, clientPwd, serverUrl, navigate]);

  function handleRemoveHistory(e: React.MouseEvent, deviceId: string) {
    e.stopPropagation();
    setHistory(removeHistory(deviceId));
  }

  return (
    <div className="app-layout">
      <aside className="sidebar">
        <div className="sidebar-brand">
          <div className="brand-hex">⬡</div>
          <span className="brand-name">RemoteDesk</span>
          <span className="brand-ver">v0.1</span>
        </div>
        <nav className="sidebar-nav">
          <button className="nav-item active">
            <span className="nav-icon">⇄</span>
            <span>远程协助</span>
          </button>
          <button className="nav-item" onClick={() => navigate("/settings")}>
            <span className="nav-icon">⚙</span>
            <span>设置</span>
          </button>
        </nav>
        <div className="sidebar-footer">
          <div className="conn-status">
            <span className="conn-dot" />
            安全加密连接
          </div>
        </div>
      </aside>

      <main className="main-content">
        <section className="panel">
          <p className="panel-title">允许控制本设备</p>
          <div className="host-body">
            <div className="id-col">
              <div className="field-label">本设备识别码</div>
              <div className="id-display">
                {hostStatus === "starting" ? (
                  <span className="id-number id-placeholder">获取中…</span>
                ) : roomId ? (
                  <>
                    <span className="id-number">{formatId(roomId)}</span>
                    <button className="id-copy-btn" onClick={copyId}>
                      {copied ? "✓ 已复制" : "复制"}
                    </button>
                  </>
                ) : (
                  <span className="id-number id-placeholder">— — — — · — — — —</span>
                )}
              </div>
            </div>
            <div className="host-divider" />
            <div className="pwd-col">
              <div className="field-label">验证码</div>
              <div className="pwd-input-wrap">
                <input
                  className="pwd-field"
                  type={showHostPwd ? "text" : "password"}
                  placeholder="—"
                  value={pwdDraft ?? hostPwd}
                  onChange={(e) => setPwdDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && pwdDraft !== null) {
                      const newPwd = pwdDraft.trim() || genPassword(6);
                      setHostPwd(newPwd);
                      setPwdDraft(null);
                      invoke("disconnect").catch(() => {});
                      setRoomId(null);
                      startHosting(newPwd, serverUrl, roomId);
                    } else if (e.key === "Escape") {
                      setPwdDraft(null);
                    }
                  }}
                  onFocus={() => setPwdDraft(hostPwd)}
                  onBlur={() => setPwdDraft(null)}
                />
                <button className="pwd-toggle" onClick={() => setShowHostPwd(!showHostPwd)} tabIndex={-1}>
                  {showHostPwd ? "◡" : "◠"}
                </button>
              </div>
              {pwdDraft !== null && <p className="pwd-hint">按 Enter 应用新验证码</p>}
            </div>
          </div>

          {hostError && (
            <p className={`error-msg ${hostStatus === "starting" ? "error-msg-retrying" : ""}`}>
              {hostError}
            </p>
          )}

          <div className="panel-actions">
            {hostStatus === "starting" && (
              <button className="btn btn-outline" disabled>创建中…</button>
            )}
            {(hostStatus === "idle" || hostStatus === "error") && (
              <button className="btn btn-outline" onClick={() => startHosting(hostPwd, serverUrl, roomId)}>
                重新连接
              </button>
            )}
            {(hostStatus === "waiting" || hostStatus === "hosting") && (
              <>
                <span className={`badge ${hostStatus === "hosting" ? "badge-connected" : "badge-waiting"}`}>
                  <span className="badge-dot" />
                  {hostStatus === "waiting" ? "等待连接" : "已连接"}
                </span>
                <button className="btn btn-ghost" onClick={stopHosting}>停止</button>
              </>
            )}
          </div>
        </section>

        <section className="panel">
          <p className="panel-title">远程控制设备</p>
          <div className="client-form">
            <input
              className="client-id-input"
              placeholder="伙伴识别码"
              value={partnerId}
              onChange={(e) => setPartnerId(e.target.value.replace(/\s/g, "").toUpperCase())}
              onKeyDown={(e) => e.key === "Enter" && connectToPartner()}
              maxLength={8}
              spellCheck={false}
            />
            <span className="client-form-sep">—</span>
            <input
              className="client-pwd-input"
              type="password"
              placeholder="验证码（可空）"
              value={clientPwd}
              onChange={(e) => setClientPwd(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && connectToPartner()}
            />
            <button
              className="btn btn-primary"
              onClick={() => connectToPartner()}
              disabled={!partnerId || clientStatus === "connecting"}
            >
              {clientStatus === "connecting" ? "连接中…" : "连接"}
            </button>
          </div>

          {clientError && <p className="error-msg">{clientError}</p>}

          {history.length > 0 && (
            <div className="history-section">
              <div className="history-label">最近连接</div>
              <div className="history-list">
                {history.map((entry) => (
                  <button
                    key={entry.deviceId}
                    className="history-item"
                    onClick={() => connectToPartner(entry.deviceId, entry.password)}
                    title={`连接到 ${entry.deviceId}`}
                  >
                    <span className="history-item-icon">⇄</span>
                    <span className="history-item-id">{formatId(entry.deviceId)}</span>
                    <span className="history-item-time">{timeAgo(entry.lastConnected)}</span>
                    <span
                      className="history-item-remove"
                      role="button"
                      onClick={(e) => handleRemoveHistory(e, entry.deviceId)}
                      title="删除"
                    >
                      ×
                    </span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </section>
      </main>
    </div>
  );
}