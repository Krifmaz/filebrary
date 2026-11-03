/**
 * Cast button for a `<video>` or `<audio>` element.
 *
 * Uses the browser's Remote Playback API, which Chrome routes to Chromecast
 * and Safari to AirPlay, so no third-party script is loaded. The button only
 * appears when the browser reports a device nearby. Give the element a
 * cookie-free source from {@link castUrl} so the device can fetch it.
 *
 * Other viewers (previews) can reuse it: `<CastButton media={ref} />`.
 */
import { useEffect, useState, type RefObject } from "react";

type CastState = "unsupported" | "unavailable" | "available" | "connecting" | "connected";

/** Props for {@link CastButton}. */
export interface CastButtonProps {
  media: RefObject<HTMLMediaElement | null>;
}

/** Shows "Cast" when a device is available, and the connection state. */
export function CastButton({ media }: CastButtonProps) {
  const [state, setState] = useState<CastState>("unsupported");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const el = media.current;
    const remote = el?.remote;
    if (!el || !remote) return;
    let id: number | undefined;
    let alive = true;
    const sync = () => {
      if (alive) setState(remote.state === "disconnected" ? "available" : remote.state);
    };
    remote.addEventListener("connecting", sync);
    remote.addEventListener("connect", sync);
    remote.addEventListener("disconnect", sync);
    remote
      .watchAvailability((available) => {
        if (!alive) return;
        if (!available) setState("unavailable");
        else sync();
      })
      .then((n) => {
        id = n;
      })
      .catch(() => {
        // Browsers that cannot watch (e.g. data saver) can still prompt on click.
        if (alive) setState("available");
      });
    return () => {
      alive = false;
      remote.removeEventListener("connecting", sync);
      remote.removeEventListener("connect", sync);
      remote.removeEventListener("disconnect", sync);
      if (id !== undefined) remote.cancelWatchAvailability(id).catch(() => {});
    };
  }, [media]);

  if (state === "unsupported" || state === "unavailable") return null;
  const label = state === "connected" ? "Casting" : state === "connecting" ? "Connecting…" : "Cast";
  return (
    <>
      <button
        type="button"
        className={state === "connected" ? "primary cast" : "cast"}
        title="Play on a Chromecast or AirPlay device"
        onClick={() => {
          setError(null);
          media.current?.remote.prompt().catch((e: unknown) => {
            // Closing the picker rejects with NotAllowedError: not an error for the user.
            if (e instanceof DOMException && e.name === "NotAllowedError") return;
            setError(e instanceof Error ? e.message : String(e));
          });
        }}
      >
        📺 {label}
      </button>
      {error && <span className="error">{error}</span>}
    </>
  );
}
