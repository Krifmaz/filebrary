# Standalone binary

Every release on the
[releases page](https://github.com/Krifmaz/filebrary/releases) has a binary
for each platform. The web interface is embedded, so the binary is all you
need.

| Platform | File |
| --- | --- |
| Linux x86_64 (static, musl) | `filebrary-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 (static, musl) | `filebrary-<version>-aarch64-unknown-linux-musl.tar.gz` |
| macOS Intel | `filebrary-<version>-x86_64-apple-darwin.tar.gz` |
| macOS Apple silicon | `filebrary-<version>-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `filebrary-<version>-x86_64-pc-windows-msvc.zip` |

Each archive has a matching `.sha256` file. Verify it before running:

```sh
sha256sum -c filebrary-<version>-x86_64-unknown-linux-musl.tar.gz.sha256
```

## Linux and macOS

```sh
tar xzf filebrary-<version>-<target>.tar.gz
sudo install -m 0755 filebrary /usr/local/bin/filebrary
FILEBRARY_DATA_DIR=~/.local/share/filebrary FILEBRARY_SOURCES=files=$HOME/Files filebrary
```

On macOS, binaries downloaded with a browser are quarantined. Clear the flag
with `xattr -d com.apple.quarantine /usr/local/bin/filebrary`.

### Running as a systemd service

```ini
# /etc/systemd/system/filebrary.service
[Unit]
Description=Filebrary file manager
After=network-online.target
Wants=network-online.target

[Service]
User=filebrary
Group=filebrary
Environment=FILEBRARY_LISTEN=127.0.0.1:8080
Environment=FILEBRARY_DATA_DIR=/var/lib/filebrary
Environment=FILEBRARY_SOURCES=files=/srv/files
ExecStart=/usr/local/bin/filebrary
Restart=on-failure
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/filebrary /srv/files

[Install]
WantedBy=multi-user.target
```

```sh
sudo useradd --system --home /var/lib/filebrary --create-home filebrary
sudo systemctl daemon-reload
sudo systemctl enable --now filebrary
```

## Windows

Unzip `filebrary-<version>-x86_64-pc-windows-msvc.zip` and run:

```powershell
$env:FILEBRARY_DATA_DIR = "$env:LOCALAPPDATA\Filebrary"
$env:FILEBRARY_SOURCES = "files=D:\Files"
.\filebrary.exe
```

Then open <http://localhost:8080>.
