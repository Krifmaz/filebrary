/**
 * Registers a video/audio player with a cast button. Its priority is low so a
 * richer previews viewer can take over and reuse {@link CastButton}.
 */
import { lazy } from "react";
import { registerViewer } from "../../viewers";
import { isCastable } from "./api";

export { CastButton } from "./CastButton";
export { castUrl, isCastable } from "./api";

registerViewer({
  id: "media-cast",
  priority: -10,
  accepts: (entry, source) => !entry.is_dir && source.permissions.download && isCastable(entry.mime),
  component: lazy(() => import("./MediaViewer")),
});
