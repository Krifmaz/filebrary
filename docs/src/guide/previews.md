# Previews and conversions

> **Planned for Phases 2 and 3.** See [Feature status](../status.md).

## Previews

| Kind | Thumbnails and preview | Needs |
|---|---|---|
| Images (JPEG, PNG, GIF, WebP, AVIF, …) | yes | built in |
| HEIC and camera RAW | yes | libheif (`-full` image) for HEIC |
| Video | thumbnail, playback with seeking | ffmpeg (`-full` image) |
| Audio | playback with seeking, cover art | built in; ffmpeg for some formats |
| PDF | page preview | poppler (`-full` image) |
| Office documents (Word, Excel, PowerPoint, OpenDocument) | converted to PDF, then previewed | LibreOffice (`-full` image) |

Thumbnails and converted previews are cached in the data directory, so each
file is converted once. Use the `-full` [Docker image](../install/docker.md)
for video, PDF, HEIC and Office previews, or install ffmpeg, poppler,
libheif and LibreOffice next to the binary.

## Conversions

Conversions run as background [jobs](jobs.md), with progress shown in the UI:

* **Documents** to PDF.
* **Video**: transcode to another codec or remux into another container.
* **Photos**: resize and change format.

## Casting

> **Planned for Phase 4.**

Video and audio players show a cast button to play on a Chromecast or other
Google Cast device on the same network.
