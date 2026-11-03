#!/usr/bin/env bash
# Runs the storage backend tests against real servers:
#   * MinIO (S3) as a local process,
#   * OpenSSH SFTP (atmoz/sftp) and Samba (dperson/samba) in Docker.
#
# Needs Docker and either a `minio` binary on PATH (or in $MINIO_BIN) or Go,
# which is used to build the pinned MinIO version below. Everything it starts
# is stopped again on exit. Extra arguments go to `cargo test`.
#
#   scripts/test-backends.sh
#   scripts/test-backends.sh -- --nocapture
set -euo pipefail

MINIO_VERSION="v0.0.0-20260212201848-7aac2a2c5b7c"
S3_PORT="${S3_PORT:-19000}"
SFTP_PORT="${SFTP_PORT:-12222}"
SMB_PORT="${SMB_PORT:-11445}"
USER_NAME="fbtest"
USER_PASS="fbtest-pass"
SFTP_CONTAINER="filebrary-test-sftp"
SMB_CONTAINER="filebrary-test-smb"

work="$(mktemp -d)"
minio_pid=""
cleanup() {
  [[ -n "$minio_pid" ]] && kill "$minio_pid" 2>/dev/null || true
  docker rm -f "$SFTP_CONTAINER" "$SMB_CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

# --- MinIO -------------------------------------------------------------------
minio="${MINIO_BIN:-$(command -v minio || true)}"
if [[ -z "$minio" ]]; then
  echo "Building MinIO $MINIO_VERSION with Go..."
  GOBIN="$work/bin" go install "github.com/minio/minio@$MINIO_VERSION"
  minio="$work/bin/minio"
fi
mkdir -p "$work/minio/fbtest" # a directory at the top level is a bucket
MINIO_ROOT_USER="$USER_NAME" MINIO_ROOT_PASSWORD="$USER_PASS" \
  "$minio" server "$work/minio" --address "127.0.0.1:$S3_PORT" --quiet &
minio_pid=$!

# --- SFTP and SMB ------------------------------------------------------------
docker rm -f "$SFTP_CONTAINER" "$SMB_CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$SFTP_CONTAINER" -p "127.0.0.1:$SFTP_PORT:22" \
  atmoz/sftp "$USER_NAME:$USER_PASS:1001::upload" >/dev/null
docker run -d --name "$SMB_CONTAINER" -p "127.0.0.1:$SMB_PORT:445" \
  dperson/samba -p -u "$USER_NAME;$USER_PASS" \
  -s "share;/share;no;no;no;$USER_NAME" >/dev/null

wait_port() {
  for _ in $(seq 1 60); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then return 0; fi
    sleep 1
  done
  echo "port $1 did not open" >&2
  return 1
}
wait_port "$S3_PORT"
wait_port "$SFTP_PORT"
wait_port "$SMB_PORT"
sleep 3 # let sshd and smbd finish starting after the port opens

# Links on the Samba share for the escape test: one inside, two leaving it.
docker exec "$SMB_CONTAINER" sh -c '
  mkdir -p /share/linktest/real &&
  echo hi > /share/linktest/real/f.txt &&
  ln -sfn real /share/linktest/inside &&
  ln -sfn /etc /share/linktest/outside &&
  ln -sfn /etc/passwd /share/linktest/pw &&
  chown -R '"$USER_NAME"' /share/linktest'

export FILEBRARY_TEST_REQUIRE_REMOTE=1
export FILEBRARY_TEST_S3_ENDPOINT="http://127.0.0.1:$S3_PORT"
export FILEBRARY_TEST_S3_BUCKET=fbtest
export FILEBRARY_TEST_S3_ACCESS_KEY="$USER_NAME"
export FILEBRARY_TEST_S3_SECRET_KEY="$USER_PASS"
export FILEBRARY_TEST_SFTP_ADDR="127.0.0.1:$SFTP_PORT"
export FILEBRARY_TEST_SFTP_USER="$USER_NAME"
export FILEBRARY_TEST_SFTP_PASSWORD="$USER_PASS"
export FILEBRARY_TEST_SFTP_ROOT=upload
export FILEBRARY_TEST_SMB_ADDR="127.0.0.1:$SMB_PORT"
export FILEBRARY_TEST_SMB_SHARE=share
export FILEBRARY_TEST_SMB_USER="$USER_NAME"
export FILEBRARY_TEST_SMB_PASSWORD="$USER_PASS"
export FILEBRARY_TEST_SMB_LINKS=linktest

cargo test -p filebrary-storage --all-features --test backends "$@"
