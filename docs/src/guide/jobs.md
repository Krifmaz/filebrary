# Jobs

Long-running work runs in a background job queue: conversions, creating
archives, and custom jobs. Each job shows its progress; a queued or running
job can be cancelled, and a failed or cancelled job can be retried. Jobs
that were running when the server stopped start again when it comes back.
Finished jobs disappear from the list after
[`jobs.retain_days`](../configuration/reference.md#jobsretain_days).

Everyone sees their own jobs; administrators see everyone's. A job always
runs with the current permissions of the user who started it, so taking
away someone's access also stops their queued jobs. Results are saved next
to the original file (or in a folder you choose) under a free name such as
`report (1).pdf`; a job never overwrites a file, except a custom job set to
`replace`. Files a job writes appear in the activity log.

How many jobs run at once is set by
[`jobs.workers`](../configuration/reference.md#jobsworkers).

## Conversions

| Conversion | Needs | Options |
|---|---|---|
| Documents, spreadsheets and presentations to PDF | LibreOffice | none |
| Video: transcode to MP4, MKV, MOV (H.264/AAC) or WebM (VP9/Opus) | ffmpeg | maximum height (never upscales), quality (CRF), audio bitrate |
| Video: remux (copy the streams into another container, fast and lossless) | ffmpeg | none |
| Audio: extract or convert to MP3, M4A (AAC) or Opus | ffmpeg | audio bitrate |
| Photos: resize and change format (JPEG, PNG, WebP, GIF, TIFF, BMP) | built in; ffmpeg for HEIC and similar | maximum width and height, JPEG quality |

Conversions need the **download** permission on the file and **upload** on
the folder the result goes to. The `-full` Docker image includes ffmpeg and
LibreOffice; with the small image, or a binary on a machine without them,
these conversions are shown as unavailable. Photo resizing works
everywhere. Point Filebrary at the tools with
[`jobs.ffmpeg`](../configuration/reference.md#jobsffmpeg) and
[`jobs.soffice`](../configuration/reference.md#jobssoffice) if they are not
on `PATH`.

## Archives

**Create archive** packs the selected files and folders into a zip or
tar.gz file in a folder you choose. It needs **download** on everything
selected and **upload** on the destination. See also
[Archives](files.md#archives) for browsing inside archives.

## Custom jobs

Administrators can define custom jobs in the configuration file under
[`[[jobs.custom]]`](../configuration/reference.md#jobscustom): a program and
its arguments, run against a selected file. They can only be defined in the
configuration file, never from the web UI or the API. Filebrary runs the
program directly, never through a shell, on a private copy of the file, so
file names cannot inject commands; the file's name is never even on the
command line. The environment is cleared, and the program (with anything it
started) is stopped on cancel or after its time limit.

```toml
# Shrink PNG files in place (users need the edit permission).
[[jobs.custom]]
name = "optimise-png"
title = "Optimise PNG"
program = "optipng"
args = ["-quiet", "-o2", "{input}"]
extensions = ["png"]
output = "replace"

# Make a 30-second preview clip next to any video.
[[jobs.custom]]
name = "preview-clip"
title = "Make preview clip"
program = "ffmpeg"
args = ["-nostdin", "-i", "{input}", "-t", "30", "-c", "copy", "{output_dir}/preview.{ext}"]
extensions = ["mp4", "mkv", "mov"]
output = "files"
```

## API

| Request | What it does |
|---|---|
| `GET /api/jobs` | Your jobs, newest first (`?status=`, `?limit=`; administrators can add `?all=true`) |
| `POST /api/jobs` | Start a job: `{"kind": "...", "params": {...}}` |
| `GET /api/jobs/kinds` | The job kinds this server offers, and whether each is available |
| `GET /api/jobs/{id}` | One job, with `status`, `progress` (0 to 1), `message`, `error` and `result` |
| `POST /api/jobs/{id}/cancel` | Cancel a queued or running job |
| `POST /api/jobs/{id}/retry` | Queue a failed or cancelled job again |
| `DELETE /api/jobs/{id}` | Remove a finished job from the list |

Job kinds and their parameters (`source` and `path` are always the source
id and the file's path; `dest` is an optional destination folder):

| Kind | Parameters |
|---|---|
| `archive.create` | `source`, `paths` (list), `dest`, `name` (optional), `format`: `zip` or `tar.gz` |
| `convert.document` | `source`, `path`, `dest` |
| `convert.video` | `source`, `path`, `dest`, `format`: `mp4`, `mkv`, `webm`, `mov`, `mp3`, `m4a`, `opus`; `mode`: `transcode` (default) or `remux`; `max_height`, `crf`, `audio_kbps` |
| `convert.image` | `source`, `path`, `dest`, `format`: `jpeg`, `png`, `webp`, `gif`, `tiff`, `bmp`; `max_width`, `max_height`, `quality` |
| `custom.<name>` | `source`, `path` |
