#!/usr/bin/env bash
# Authenticode signature of Windows files (.exe, .msi) with jsign and the DentalCraft
# self-signed certificate, an RFC 3161 timestamp, then a check of every signature and
# timestamp with osslsigncode. Existing signatures are replaced, so a rerun on a file that
# is already signed leaves one signature.
#
# Windows does not trust a self-signed certificate: the signature proves which key signed
# the files (its fingerprint is in the release notes), but SmartScreen still warns and
# shows an unknown publisher.
#
#   [AUTHENTICODE_KEY_DIR=<dir>] [AUTHENTICODE_REQUIRED=1] \
#     bash packaging/windows/authenticode.sh FILE...
#
# binaries.yml runs it twice, and so does `scripts/release-local.sh binaries app` of the
# dentalcraft superproject: on dentalcraft.exe and dentalcraft-cli.exe after the build,
# before they are packed (zip, setup.exe, MSI), then on setup.exe and the MSI. Both runs
# come before update.json and the checksums, which hash the signed files.
#
# AUTHENTICODE_KEY_DIR is the directory of the signing key (locally
# ~/.config/dentalcraft-release/authenticode, mounted read-only; on the runner a 0600 copy
# of the secrets under $RUNNER_TEMP):
#   dentalcraft-selfsigned.p12  PKCS#12 with the key and the certificate, alias dentalcraft
#   storepass                   its password (jsign --storepass file:, never on a command line)
# The signatures must chain to the pinned public certificate dentalcraft-selfsigned.crt.pem
# next to this script; AUTHENTICODE_CAFILE replaces it (tests with another key).
# Without a key (AUTHENTICODE_KEY_DIR unset or empty) it prints a warning and leaves the
# files unsigned; with AUTHENTICODE_REQUIRED=1 it fails. A key directory that is missing
# or incomplete is always an error.
# The timestamp authority is SSL.com's (AUTHENTICODE_TSA overrides it,
# AUTHENTICODE_TSA_CAFILE its trust anchor).
# Needs curl; java (8 or later) and osslsigncode are installed with apt-get or dnf when
# missing, and only when there is a key.
set -euo pipefail

# jsign, pinned with its checksum (the same file as the GitHub release asset 7.6).
JSIGN_URL=https://repo1.maven.org/maven2/net/jsign/jsign/7.6/jsign-7.6.jar
JSIGN_SHA256=6cf58aa080e7ff82656f320cff8d4a3bff30408df8766806dcba3740dd839fa5
TSA_URL=${AUTHENTICODE_TSA:-http://ts.ssl.com}
here=$(dirname "${BASH_SOURCE[0]}")
# The root of the ts.ssl.com timestamps (https://www.ssl.com/repository/), sha256:
#   SSL.com Root Certification Authority RSA  85666A562EE0BE5CE925C1D8890A6F76A87EC16D4D7D5F29EA7419CF20123B69
TSA_ROOT=$here/timestamp-root.pem
# The self-signed signing certificate, sha256 in the file.
SIGNING_CERT=$here/dentalcraft-selfsigned.crt.pem
KEY_ALIAS=dentalcraft

die() { printf '##[error]%s\n' "$*" >&2; exit 1; }
warn() { printf '##[warning]%s\n' "$*" >&2; }

[ $# -gt 0 ] || die "usage: authenticode.sh FILE..."
for f in "$@"; do
  case "$f" in *.exe|*.msi) ;; *) die "not an .exe or .msi file: $f" ;; esac
  [ -f "$f" ] || die "no such file: $f"
done

if [ -z "${AUTHENTICODE_KEY_DIR:-}" ]; then
  [ "${AUTHENTICODE_REQUIRED:-}" != 1 ] || die "no Authenticode key (AUTHENTICODE_KEY_DIR) and AUTHENTICODE_REQUIRED=1: refusing to leave $* unsigned"
  warn "no Authenticode key (AUTHENTICODE_KEY_DIR): $* stay unsigned (no Authenticode signature)"
  exit 0
fi

# ---------------------------------------------------------------- the key

keystore=$AUTHENTICODE_KEY_DIR/dentalcraft-selfsigned.p12
storepass=$AUTHENTICODE_KEY_DIR/storepass
[ -d "$AUTHENTICODE_KEY_DIR" ] || die "AUTHENTICODE_KEY_DIR: no such directory: $AUTHENTICODE_KEY_DIR"
[ -s "$keystore" ] || die "AUTHENTICODE_KEY_DIR: no dentalcraft-selfsigned.p12 in $AUTHENTICODE_KEY_DIR"
[ -s "$storepass" ] || die "AUTHENTICODE_KEY_DIR: no storepass (the password of the .p12) in $AUTHENTICODE_KEY_DIR"
cafile=${AUTHENTICODE_CAFILE:-$SIGNING_CERT}
[ -f "$cafile" ] || die "AUTHENTICODE_CAFILE: no such file: $cafile"
tsa_cafile=${AUTHENTICODE_TSA_CAFILE:-$TSA_ROOT}
[ -f "$tsa_cafile" ] || die "AUTHENTICODE_TSA_CAFILE: no such file: $tsa_cafile"

# ---------------------------------------------------------------- tools

printf '##[group]Authenticode tools\n'
need=()
command -v curl >/dev/null || need+=(curl)
command -v java >/dev/null || need+=(java)
command -v osslsigncode >/dev/null || need+=(osslsigncode)
if [ ${#need[@]} -gt 0 ]; then
  SUDO=""; [ "$(id -u)" -ne 0 ] && SUDO="sudo"
  if command -v apt-get >/dev/null; then
    $SUDO apt-get update -qq
    $SUDO apt-get install -y -qq --no-install-recommends ca-certificates curl default-jre-headless osslsigncode >/dev/null
  elif command -v dnf >/dev/null; then
    $SUDO dnf install -y -q curl java-25-openjdk-headless osslsigncode >/dev/null
  else
    die "no supported package manager to install ${need[*]}"
  fi
fi
java -version 2>&1 | head -n 1
osslsigncode --version | head -n 1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
jar=$work/${JSIGN_URL##*/}
curl -fsSL --retry 3 -o "$jar" "$JSIGN_URL"
printf '%s  %s\n' "$JSIGN_SHA256" "$jar" | sha256sum -c --quiet - || die "checksum mismatch for $JSIGN_URL"
echo "jsign ${JSIGN_URL##*/} (sha256 $JSIGN_SHA256)"
printf '##[endgroup]\n'

# ---------------------------------------------------------------- sign

printf '##[group]Sign (self-signed, timestamp %s)\n' "$TSA_URL"
java -jar "$jar" --storetype PKCS12 --keystore "$keystore" --storepass "file:$storepass" \
  --alias "$KEY_ALIAS" --alg SHA-256 --tsaurl "$TSA_URL" --tsmode RFC3161 \
  --replace --name DentalCraft --url https://dentalcraft.app "$@" \
  || die "jsign failed: $* are not signed"
printf '##[endgroup]\n'

# ---------------------------------------------------------------- verify

# osslsigncode exits 0 when the timestamp fails but the signature verifies at the current
# time: require the timestamp, and exactly one signature, explicitly.
for f in "$@"; do
  printf '##[group]Verify %s\n' "$f"
  out=$(osslsigncode verify -in "$f" -CAfile "$cafile" -TSA-CAfile "$tsa_cafile" 2>&1) \
    || { printf '%s\n' "$out"; die "$f: the Authenticode signature does not verify"; }
  printf '%s\n' "$out" | grep -E 'Subject:|Timestamp time:|verification:|Number of verified signatures:'
  printf '%s\n' "$out" | grep -qx 'Signature verification: ok' || die "$f: the signature does not verify"
  printf '%s\n' "$out" | grep -qx 'Timestamp Server Signature verification: ok' || die "$f: the timestamp does not verify"
  printf '%s\n' "$out" | grep -qx 'Number of verified signatures: 1' || die "$f: not exactly one signature"
  printf '##[endgroup]\n'
  echo "signed and timestamped: $f"
done
