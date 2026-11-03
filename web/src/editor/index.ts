/**
 * Registers the text editor as the viewer for text and code files.
 *
 * The editor and CodeMirror load lazily, only when a file is opened; each
 * language's highlighting loads on demand after that.
 */
import { lazy } from "react";
import { registerViewer } from "../viewers";
import { isTextFile } from "./textFiles";

registerViewer({
  id: "editor",
  // Below dedicated previews (images, PDF, Markdown rendering...), which can
  // still offer an "edit" action that opens this viewer.
  priority: -10,
  accepts: (entry, source) => source.permissions.download && isTextFile(entry),
  component: lazy(() => import("./Editor")),
});
