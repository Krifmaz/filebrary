/**
 * Full-screen CodeMirror editor for text and code files.
 *
 * Opens through the viewer registry (see `./index.ts`). It loads the file
 * with its ETag and saves with `If-Match`, so a change made elsewhere in the
 * meantime (another tab, another user, WebDAV, the disk itself) is reported
 * as a conflict instead of being overwritten. Users without the `edit`
 * permission get a read-only view.
 *
 * Shortcuts: Ctrl/Cmd+S saves; the usual CodeMirror keys (Ctrl/Cmd+F
 * search, Ctrl/Cmd+Z undo, Tab indents) work as well.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { basicSetup } from "codemirror";
import { Compartment, EditorState, Prec, type Text } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import { HighlightStyle, LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { tags as t } from "@lezer/highlight";
import { ApiError } from "../api";
import type { ViewerProps } from "../viewers";
import { loadText, SaveConflict, saveText } from "./api";
import "./editor.css";

type Status =
  | { kind: "loading" }
  | { kind: "error"; message: string; code?: string }
  | { kind: "ready" };

type SaveState =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved" }
  | { kind: "failed"; message: string }
  | { kind: "conflict"; message: string; currentEtag: string | null };

const WRAP_KEY = "filebrary.editor.wrap";

function readWrap(): boolean {
  try {
    return localStorage.getItem(WRAP_KEY) !== "0";
  } catch {
    return true;
  }
}

function storeWrap(on: boolean): void {
  try {
    localStorage.setItem(WRAP_KEY, on ? "1" : "0");
  } catch {
    /* storage unavailable: keep the choice for this session only */
  }
}

/** Colours come from CSS variables in `editor.css`, so light and dark both work. */
const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword], color: "var(--cm-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: "var(--cm-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--cm-number)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--cm-comment)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: "var(--cm-function)" },
  { tag: [t.typeName, t.className, t.namespace, t.tagName], color: "var(--cm-type)" },
  { tag: [t.propertyName, t.attributeName], color: "var(--cm-property)" },
  { tag: [t.definition(t.variableName), t.labelName], color: "var(--cm-definition)" },
  { tag: [t.meta, t.annotation, t.processingInstruction], color: "var(--cm-meta)" },
  { tag: t.heading, color: "var(--cm-keyword)", fontWeight: "700" },
  { tag: t.strong, fontWeight: "700" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: [t.link, t.url], color: "var(--cm-string)", textDecoration: "underline" },
  { tag: t.invalid, color: "var(--danger)" },
]);

const theme = EditorView.theme({
  "&": { height: "100%", backgroundColor: "var(--surface)", color: "var(--text)" },
  ".cm-scroller": { fontFamily: "var(--cm-font)", lineHeight: "1.55" },
  ".cm-content": { caretColor: "var(--accent)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "var(--cm-selection)",
  },
  ".cm-gutters": { backgroundColor: "var(--surface)", color: "var(--muted)", borderRight: "1px solid var(--border)" },
  ".cm-activeLine": { backgroundColor: "var(--cm-active-line)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--cm-active-line)", color: "var(--text)" },
  ".cm-panels": { backgroundColor: "var(--surface-2)", color: "var(--text)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border)" },
  ".cm-searchMatch": { backgroundColor: "var(--cm-match)", outline: "1px solid var(--accent)" },
  ".cm-tooltip": { backgroundColor: "var(--surface)", border: "1px solid var(--border)", color: "var(--text)" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--accent-soft)", color: "var(--text)" },
});

/** The CodeMirror editor viewer. */
export default function Editor({ source, entry, onClose, onChanged }: ViewerProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const etag = useRef<string | null>(null);
  const savedDoc = useRef<Text | null>(null);
  const langConf = useRef(new Compartment());
  const wrapConf = useRef(new Compartment());
  const saveRef = useRef<() => void>(() => {});

  const readOnly = !source.permissions.edit;
  const [status, setStatus] = useState<Status>({ kind: "loading" });
  const [save, setSave] = useState<SaveState>({ kind: "idle" });
  const [dirty, setDirty] = useState(false);
  const [wrap, setWrap] = useState(readWrap);
  const [cursor, setCursor] = useState({ line: 1, col: 1 });
  const [language, setLanguage] = useState<string>("Plain text");
  const [reloadKey, setReloadKey] = useState(0);

  // Load the file and build the editor. Re-runs on "Reload".
  useEffect(() => {
    const ctrl = new AbortController();
    let created: EditorView | null = null;
    loadText(source.id, entry.path, ctrl.signal)
      .then((file) => {
        if (ctrl.signal.aborted || !host.current) return;
        etag.current = file.etag;
        const state = EditorState.create({
          doc: file.content,
          extensions: [
            Prec.highest(
              keymap.of([
                {
                  key: "Mod-s",
                  preventDefault: true,
                  run: () => {
                    saveRef.current();
                    return true;
                  },
                },
              ]),
            ),
            syntaxHighlighting(highlight),
            basicSetup,
            keymap.of([indentWithTab]),
            theme,
            langConf.current.of([]),
            wrapConf.current.of(readWrap() ? EditorView.lineWrapping : []),
            EditorState.readOnly.of(readOnly),
            EditorView.contentAttributes.of({ "aria-label": `Contents of ${entry.name}` }),
            EditorView.updateListener.of((u) => {
              if (u.docChanged) {
                setDirty(savedDoc.current ? !u.state.doc.eq(savedDoc.current) : true);
                setSave((s) => (s.kind === "saved" ? { kind: "idle" } : s));
              }
              if (u.docChanged || u.selectionSet) {
                const head = u.state.selection.main.head;
                const line = u.state.doc.lineAt(head);
                setCursor({ line: line.number, col: head - line.from + 1 });
              }
            }),
          ],
        });
        savedDoc.current = state.doc;
        const v = new EditorView({ state, parent: host.current });
        created = v;
        view.current = v;
        setStatus({ kind: "ready" });
        if (!readOnly) v.focus();

        const desc = LanguageDescription.matchFilename(languages, entry.name);
        if (desc) {
          desc
            .load()
            .then((support) => {
              if (view.current !== v) return;
              v.dispatch({ effects: langConf.current.reconfigure(support) });
              setLanguage(desc.name);
            })
            .catch(() => {
              /* highlighting is optional; keep plain text */
            });
        } else {
          setLanguage("Plain text");
        }
      })
      .catch((e: unknown) => {
        if (ctrl.signal.aborted) return;
        if (e instanceof ApiError) setStatus({ kind: "error", message: e.message, code: e.code });
        else setStatus({ kind: "error", message: "The file could not be opened" });
      });
    return () => {
      ctrl.abort();
      created?.destroy();
      if (view.current === created) view.current = null;
    };
  }, [source.id, entry.path, entry.name, readOnly, reloadKey]);

  /** Saves the current text; `base` overrides the ETag (for "overwrite"). */
  const doSave = useCallback(
    async (base?: string | null) => {
      const v = view.current;
      if (!v || readOnly) return;
      const doc = v.state.doc;
      setSave({ kind: "saving" });
      try {
        const res = await saveText(source.id, entry.path, doc.toString(), base === undefined ? etag.current : base);
        etag.current = res.etag;
        savedDoc.current = doc;
        setDirty(!v.state.doc.eq(doc));
        setSave({ kind: "saved" });
        onChanged();
      } catch (e) {
        if (e instanceof SaveConflict) setSave({ kind: "conflict", message: e.message, currentEtag: e.currentEtag });
        else setSave({ kind: "failed", message: e instanceof Error ? e.message : "Save failed" });
      }
    },
    [source.id, entry.path, readOnly, onChanged],
  );

  useEffect(() => {
    saveRef.current = () => {
      if (save.kind !== "saving") void doSave();
    };
  }, [doSave, save.kind]);

  // Warn before leaving the page with unsaved changes.
  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => {
      e.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  const toggleWrap = () => {
    const next = !wrap;
    setWrap(next);
    storeWrap(next);
    view.current?.dispatch({ effects: wrapConf.current.reconfigure(next ? EditorView.lineWrapping : []) });
  };

  const close = () => {
    if (dirty && !window.confirm("Discard your unsaved changes?")) return;
    onClose();
  };

  const reload = () => {
    if (dirty && !window.confirm("Reload the file and discard your changes?")) return;
    setStatus({ kind: "loading" });
    setSave({ kind: "idle" });
    setDirty(false);
    setReloadKey((k) => k + 1);
  };

  return (
    <div className="editor">
      <header className="editor-bar">
        <button type="button" className="ghost" onClick={close} aria-label="Close editor">
          ← Back
        </button>
        <h2 className="editor-title" title={entry.path}>
          {entry.name}
          {dirty && (
            <span className="editor-dirty" aria-label="unsaved changes">
              {" "}
              •
            </span>
          )}
        </h2>
        <span className="editor-status" role="status" aria-live="polite">
          {readOnly && status.kind === "ready" && "Read only"}
          {save.kind === "saving" && "Saving…"}
          {save.kind === "saved" && !dirty && "Saved"}
          {save.kind === "failed" && <span className="editor-error">{save.message}</span>}
        </span>
        <button type="button" className="ghost" onClick={toggleWrap} aria-pressed={wrap} disabled={status.kind !== "ready"}>
          Wrap
        </button>
        {!readOnly && (
          <button
            type="button"
            className="primary"
            onClick={() => void doSave()}
            disabled={status.kind !== "ready" || !dirty || save.kind === "saving"}
            title="Save (Ctrl+S)"
          >
            Save
          </button>
        )}
      </header>

      {save.kind === "conflict" && (
        <div className="editor-conflict" role="alert">
          <span>
            {save.message}. Saving now would replace the other version.
          </span>
          <span className="editor-conflict-actions">
            <button type="button" onClick={reload}>
              Load their version
            </button>
            <button type="button" className="danger" onClick={() => void doSave(save.currentEtag)}>
              Overwrite with mine
            </button>
            <button type="button" className="ghost" onClick={() => setSave({ kind: "idle" })}>
              Keep editing
            </button>
          </span>
        </div>
      )}

      {status.kind === "loading" && <p className="muted center">Loading…</p>}
      {status.kind === "error" && (
        <div className="editor-message">
          <p>
            {status.code === "not_text"
              ? "This file isn't plain text, so it can't be edited here."
              : status.code === "payload_too_large"
                ? "This file is too large to edit in the browser (the limit is 5 MiB)."
                : status.message}
          </p>
          <button type="button" onClick={onClose}>
            Back to files
          </button>
        </div>
      )}
      <div ref={host} className="editor-host" hidden={status.kind !== "ready"} />

      {status.kind === "ready" && (
        <footer className="editor-foot">
          <span>
            Ln {cursor.line}, Col {cursor.col}
          </span>
          <span>{language}</span>
        </footer>
      )}
    </div>
  );
}
