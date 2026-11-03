# Configuration overview

Filebrary needs no configuration to start: with none, it serves
`./data/files` on `127.0.0.1:8080`. Settings come from three places, and
later ones win:

1. **Built-in defaults.**
2. **A TOML file**, given with `--config <file>` (or `-c <file>`) or the
   `FILEBRARY_CONFIG` environment variable. Unknown keys are an error, so a
   typo stops the server instead of being silently ignored.
3. **Environment variables** named `FILEBRARY_*`. Only some settings have
   one; the [settings reference](reference.md) lists them. This is the usual
   way to configure the Docker image.

```sh
filebrary --config /etc/filebrary/filebrary.toml
# or
FILEBRARY_CONFIG=/etc/filebrary/filebrary.toml filebrary
```

A short file:

```toml
listen = "0.0.0.0:8080"
data_dir = "/data"
public_url = "https://files.example.com"

[[sources]]
name = "files"
path = "/srv"
```

The configuration is checked at start-up. If anything is wrong (an invalid
or duplicate source name, a value out of range, an unreadable file),
Filebrary prints the reason and exits with a non-zero status, so mistakes
show up immediately in `docker logs` or `journalctl`.

* Every setting, its type, default and environment variable:
  [Settings reference](reference.md).
* A complete commented file to copy: [Example configuration](example.md).

## Docker defaults

The Docker image sets three variables, so the defaults inside a container
differ from a bare binary:

| Variable             | Value in the image | Binary default        |
|----------------------|--------------------|-----------------------|
| `FILEBRARY_LISTEN`   | `0.0.0.0:8080`     | `127.0.0.1:8080`      |
| `FILEBRARY_DATA_DIR` | `/data`            | `./data`              |
| `FILEBRARY_SOURCES`  | `files=/srv`       | `files=<data_dir>/files` |

Environment variables beat the file, so while the image's
`FILEBRARY_SOURCES` is in effect, `[[sources]]` in a mounted TOML file is
ignored. Either list your sources in the variable
(`-e FILEBRARY_SOURCES="files=/srv;media=/mnt/media"`), or clear it with
`-e FILEBRARY_SOURCES=` so the file's `[[sources]]` are used. An empty value
counts as unset.

## Command line

```text
filebrary [--config <FILE>] [COMMAND]
```

| Command       | What it does                                                                                     |
|---------------|--------------------------------------------------------------------------------------------------|
| `serve`       | Runs the server. This is the default when no command is given.                                   |
| `healthcheck` | Requests `/healthz` on the configured [`listen`](reference.md#listen) address and exits `0` if it answers `200`. Used by the Docker `HEALTHCHECK`, because the image has no `curl`. |

`filebrary --help` and `filebrary --version` work as usual. There are no
flags for individual settings; use the file or environment variables.

## Environment variables that are not settings

| Variable           | Meaning                                                                                                    |
|--------------------|------------------------------------------------------------------------------------------------------------|
| `FILEBRARY_CONFIG` | Path to the TOML file, same as `--config`.                                                                 |
| `FILEBRARY_LOG`    | Log filter in [`EnvFilter` syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html), for example `debug` or `info,filebrary=trace`. Default: `info,tower_http=info,sqlx=warn`. |

## Value formats

* **Durations** are whole numbers, with the unit in the key name
  (`_hours`, `_secs`).
* **Sizes** are whole numbers of MiB (1 MiB = 1,048,576 bytes), again named
  in the key (`_mib`).
* **Booleans** are `true` or `false` in TOML. In environment variables,
  `true`, `yes`, `on`, `1` and `false`, `no`, `off`, `0` are accepted, in
  any letter case.
* **Paths** are relative to the working directory of the process unless
  they are absolute.
