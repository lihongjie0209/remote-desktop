import { useState } from "react";

/**
 * Maps raw tonic/network error strings to user-friendly Chinese messages.
 */
export function toFriendlyMessage(raw: string): string {
  const r = raw.toLowerCase();
  if (r.includes("h2 protocol error") || r.includes("error reading a body")) {
    return "与服务器的连接中断（网络波动或服务器重启）";
  }
  if (r.includes("httpsuriwithouttlssupport") || r.includes("tls") && r.includes("transport")) {
    return "TLS 连接配置错误";
  }
  if (r.includes("transport error") || r.includes("connection refused")) {
    return "无法连接到服务器（请检查网络或服务器地址）";
  }
  if (r.includes("timed out") || r.includes("timeout")) {
    return "连接超时，请稍后重试";
  }
  if (r.includes("join rejected") || r.includes("permission denied") || r.includes("unauthenticated")) {
    return "验证码错误或设备不存在";
  }
  if (r.includes("not found") || r.includes("no host")) {
    return "目标设备不存在或未上线";
  }
  if (r.includes("room full") || r.includes("already connected")) {
    return "该设备已有其他客户端连接";
  }
  if (r.includes("重试")) {
    // Already a friendly retry message — return as-is
    return raw;
  }
  // Trim noisy tonic prefix: "status: Internal, message: "..." ..."
  const msgMatch = raw.match(/message:\s*"([^"]+)"/);
  if (msgMatch) return msgMatch[1];
  return "连接出现错误";
}

interface Props {
  /** Raw error string (from Rust/tonic) */
  error: string;
  /** Extra CSS class applied to the wrapper */
  className?: string;
}

/**
 * Displays a user-friendly Chinese error with an expandable detail panel
 * showing the raw technical error message.
 */
export default function ErrorDisplay({ error, className = "" }: Props) {
  const [expanded, setExpanded] = useState(false);
  const friendly = toFriendlyMessage(error);
  // Only show "查看详情" when the friendly message differs from raw
  const hasDetail = friendly !== error;

  return (
    <div className={`error-display ${className}`}>
      <span className="error-display-msg">{friendly}</span>
      {hasDetail && (
        <button
          className="error-display-detail-btn"
          onClick={() => setExpanded((v) => !v)}
          title={expanded ? "收起详情" : "查看详情"}
        >
          {expanded ? "收起" : "查看详情"}
        </button>
      )}
      {expanded && (
        <pre className="error-display-detail">{error}</pre>
      )}
    </div>
  );
}
