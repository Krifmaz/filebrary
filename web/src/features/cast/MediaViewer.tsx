/**
 * Full-screen player for video and audio with a cast button. The media plays
 * from a short-lived cookie-free URL so a cast device can take it over.
 */
import { useEffect, useRef, useState } from "react";
import type { ViewerProps } from "../../viewers";
import { CastButton } from "./CastButton";
import { castUrl } from "./api";

/** The viewer component registered for video and audio. */
export default function MediaViewer({ source, entry, onClose }: ViewerProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLMediaElement | null>(null);
  const isVideo = entry.mime?.startsWith("video/") ?? false;

  useEffect(() => {
    let alive = true;
    castUrl(source.id, entry.path)
      .then((u) => alive && setSrc(u.url))
      .catch((e: unknown) => alive && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      alive = false;
    };
  }, [source.id, entry.path]);

  return (
    <>
      <div className="viewer-bar">
        <strong className="grow">{entry.name}</strong>
        {src && <CastButton media={ref} />}
        <button type="button" onClick={onClose} autoFocus>Close</button>
      </div>
      <div className="viewer-body media">
        {error && <p className="error" role="alert">{error}</p>}
        {src &&
          (isVideo ? (
            <video ref={(el) => void (ref.current = el)} src={src} controls autoPlay playsInline preload="metadata" />
          ) : (
            <audio ref={(el) => void (ref.current = el)} src={src} controls autoPlay preload="metadata" />
          ))}
      </div>
    </>
  );
}
