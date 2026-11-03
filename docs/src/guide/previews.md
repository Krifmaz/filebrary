# Previews and conversions

> **Previews and conversions are planned for Phases 2 and 3; casting is
> already available.** See [Feature status](../status.md).

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

Video and audio open in a player with a **Cast** button when a Chromecast
(in Chrome) or AirPlay device (in Safari) is nearby. Casting uses the
browser's Remote Playback API, so no third-party script is loaded. The
device streams from a signed URL that expires after 6 hours and stops
working as soon as the user loses access to the file. Set
[`public_url`](../configuration/reference.md#public_url) so the device can
reach the right address.
