# Previews and conversions

Filebrary makes thumbnails for the file grid, larger previews for formats the
browser cannot show itself, and browser-playable streams for video and audio
it cannot play. Everything is generated on first request and cached.

## What gets a preview

| Kind | Thumbnail | Large preview | Needs |
|---|---|---|---|
| JPEG, PNG, GIF, WebP, BMP, ICO | resized | the original file | built in |
| TIFF | resized | converted | built in |
| Camera RAW (CR2, CR3, NEF, ARW, DNG, RAF, ORF, RW2, PEF, ...) | the JPEG the camera embedded | same, larger | built in |
| HEIC, HEIF, AVIF | decoded and resized | decoded and resized | `heif-dec` or `ffmpeg` |
| Video | a frame 10% in (at most one minute) | that frame, larger | `ffmpeg`, `ffprobe` |
| Audio | embedded cover art | cover art, larger | `ffmpeg`, `ffprobe` |
| PDF | first page | the original file (browser viewer) | `pdftoppm` |
| Word, Excel, PowerPoint, OpenDocument | first page | a PDF conversion | `soffice` (LibreOffice), `pdftoppm` |

"Built in" formats are decoded in pure Rust and work in every build. The
external tools are looked up on `PATH` at start-up and the result is logged
(`preview tools ffmpeg=yes ...`). The `-full` [Docker image](../install/docker.md)
includes all of them; the slim image includes none, so it previews images and
RAW files only. Without Docker, install `ffmpeg`, `poppler-utils`,
`libreoffice` and `libheif` tools from your package manager.

## Video and audio playback

Formats the browser plays (H.264, VP9, AV1, AAC, MP3, Opus, FLAC, ...) are
played straight from the download route, which supports HTTP range requests,
so seeking works without any conversion. For the rest (HEVC, MPEG-4 Part 2,
AC-3, DTS, WMA, ...) the player uses a live transcode to H.264/AAC in
fragmented MP4, at most 720p. Streams that are already H.264 or AAC are copied
instead of re-encoded. Seeking restarts the stream at the chosen time.

## Settings

All settings live in the `[previews]` table; see the
[settings reference](../configuration/reference.md#previews).

| Setting | Env variable | Default | Meaning |
|---|---|---|---|
| `enabled` | `FILEBRARY_PREVIEWS` | `true` | Turn previews off entirely. |
| `cache_mib` | `FILEBRARY_PREVIEW_CACHE_MIB` | `2048` | Size limit of the cache, in MiB. The least recently used previews are deleted past it (checked every 10 minutes). |
| `thumb_size` | | `256` | Longest side of thumbnails, 32-1024 px. |
| `large_size` | | `1920` | Longest side of large previews, 256-8192 px. |
| `max_parallel` | | `0` | Previews generated at once. `0` means one per CPU, at most 8. LibreOffice conversions always run one at a time. |
| `max_remote_mib` | | `1024` | Largest file from a remote source (S3, SFTP, SMB) copied locally to make a preview. |

```toml
[previews]
cache_mib = 4096
thumb_size = 320
```

## Cache

Previews are stored in `<data_dir>/cache/previews`. Each entry is keyed by the
source, the file's path, its size, its modification time and the preview
variant, so editing or replacing a file produces a new preview and the old
one ages out. A file that fails to preview (corrupt, unsupported codec, tool
timeout) is not retried for an hour. The whole `cache` folder can be deleted
at any time while the server is stopped; it is not part of backups.

## Safety

* Previews need the *download* permission on the file, and they read it
  through the same scope checks as downloads, so a symlink cannot reach outside
  the user's folder.
* External tools are started directly with an argument list, never through a
  shell, with a time limit (60 s, 180 s for LibreOffice). They are killed when
  the request is cancelled.
* Images are decoded with size and memory limits, and each kind has a file
  size limit (256 MiB images, 512 MiB RAW, 100 MiB Office).
* LibreOffice runs with a fresh, throwaway profile for every conversion.
  Scratch files go to `<data_dir>/cache/tmp`, so a read-only root file system
  is fine.

## API

| Route | Returns |
|---|---|
| `GET /api/previews` | which kinds this server can preview, and the sizes |
| `GET /api/sources/{id}/preview?path=&variant=thumb\|large\|pdf` | an image or PDF, with `ETag` |
| `GET /api/sources/{id}/media?path=` | duration, codecs, tags and `browser_playable` |
| `GET /api/sources/{id}/stream?path=&start=&audio=` | fragmented MP4 from `start` seconds |

A file without a preview answers `404` with `{"error": "no_preview"}`.

## Conversions

> **Planned for Phase 3.** See [Feature status](../status.md).

Conversions run as background [jobs](jobs.md), with progress shown in the UI:

* **Documents** to PDF.
* **Video**: transcode to another codec or remux into another container.
* **Photos**: resize and change format.

## Casting

Video and audio open in a player with a **Cast** button when a Chromecast
(in Chrome) or AirPlay device (in Safari) is nearby. Casting uses the
browser's Remote Playback API, so no third-party script is loaded. The
device streams from a signed URL that expires after 6 hours and stops
working as soon as the user loses access to the file. Set
[`public_url`](../configuration/reference.md#public_url) so the device can
reach the right address.
