/**
 * Decides which files the text editor opens.
 *
 * The browser only knows a file's name, size and guessed MIME type, so this
 * is a best guess; the server makes the final call and answers `415
 * not_text` for anything that is not UTF-8 text.
 */
import type { Entry } from "../api";

/** Largest file the editor opens, matching the server's limit (5 MiB). */
export const MAX_TEXT_BYTES = 5 * 1024 * 1024;

/** MIME types outside `text/*` that are still text. */
const TEXT_MIMES = new Set([
  "application/json",
  "application/ld+json",
  "application/manifest+json",
  "application/javascript",
  "application/x-javascript",
  "application/ecmascript",
  "application/typescript",
  "application/xml",
  "application/xhtml+xml",
  "application/x-sh",
  "application/x-shellscript",
  "application/x-httpd-php",
  "application/sql",
  "application/toml",
  "application/yaml",
  "application/x-yaml",
  "application/graphql",
  "application/x-tex",
  "application/x-latex",
  "application/x-subrip",
  "image/svg+xml",
]);

/** Extensions of text and code files that MIME guessing often misses. */
const TEXT_EXTENSIONS = new Set(
  (
    "txt text md markdown mdx rst adoc asciidoc org log csv tsv " +
    "json jsonc json5 ndjson yaml yml toml ini cfg conf config env properties editorconfig " +
    "xml xsd xsl svg html htm xhtml css scss sass less vue svelte astro " +
    "js mjs cjs jsx ts mts cts tsx " +
    "rs go c h cc cpp cxx hpp hh hxx m mm java kt kts scala groovy gradle cs fs fsx vb swift dart " +
    "py pyi rb php pl pm lua r jl ex exs erl hrl hs elm clj cljs edn lisp el scm ml mli nim zig v d " +
    "sh bash zsh fish ps1 psm1 bat cmd sql graphql gql proto thrift tf hcl nix " +
    "tex bib sty cls srt vtt diff patch gitignore gitattributes dockerignore lock"
  ).split(" "),
);

/** File names (without extension) that are text by convention. */
const TEXT_NAMES = new Set([
  "dockerfile",
  "containerfile",
  "makefile",
  "gnumakefile",
  "justfile",
  "rakefile",
  "gemfile",
  "procfile",
  "vagrantfile",
  "readme",
  "license",
  "licence",
  "copying",
  "authors",
  "changelog",
  "notice",
  "todo",
  "cmakelists.txt",
]);

/** Returns the lower-case extension of a file name, without the dot. */
export function extension(name: string): string {
  const base = name.toLowerCase();
  const dot = base.lastIndexOf(".");
  // ".bashrc" has no extension but a dot-name; treat the rest as the extension.
  return dot >= 0 ? base.slice(dot + 1) : "";
}

/** Returns true if the editor should offer to open this entry. */
export function isTextFile(entry: Pick<Entry, "name" | "is_dir" | "size" | "mime">): boolean {
  if (entry.is_dir || entry.size > MAX_TEXT_BYTES) return false;
  const mime = (entry.mime ?? "").toLowerCase().split(";")[0]!.trim();
  if (mime.startsWith("text/") || TEXT_MIMES.has(mime) || mime.endsWith("+json") || mime.endsWith("+xml")) {
    return true;
  }
  const name = entry.name.toLowerCase();
  if (TEXT_NAMES.has(name)) return true;
  return TEXT_EXTENSIONS.has(extension(name));
}
