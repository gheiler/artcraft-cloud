#!/usr/bin/env bash
# Checks deploy/cloud-init/artcraft-cloud.yaml without a VPS: the YAML parses, the embedded files
# parse, install.sh passes shellcheck (when installed), and a dry run with stubbed docker, ufw,
# systemctl, ip and curl writes the expected .env and Caddyfile, with and without a domain.
# Usage: deploy/cloud-init/check.sh   (needs python3 with PyYAML)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

python3 - "$here/artcraft-cloud.yaml" "$work" <<'PY'
import os, sys, yaml
src, out = sys.argv[1], sys.argv[2]
with open(src) as f:
    first = f.readline()
    assert first.strip() == "#cloud-config", "first line must be #cloud-config"
    doc = yaml.safe_load(first + f.read())
files = {w["path"]: w["content"] for w in doc["write_files"]}
for need in ("/opt/artcraft/.env", "/opt/artcraft/compose.yaml", "/opt/artcraft/install.sh"):
    assert need in files, f"missing {need}"
compose = yaml.safe_load(files["/opt/artcraft/compose.yaml"])
assert {"artcraft-cloud", "caddy", "stalwart"} <= set(compose["services"]), "compose services"
assert compose["services"]["stalwart"]["profiles"] == ["mail"]
assert doc["runcmd"] == [["bash", "/opt/artcraft/install.sh"]]
os.makedirs(os.path.join(out, "root"))
for path, body in files.items():
    with open(os.path.join(out, "root", os.path.basename(path)), "w") as f:
        f.write(body)
PY

if command -v shellcheck > /dev/null; then
  shellcheck -s bash "$work/root/install.sh"
else
  echo "shellcheck not installed, skipped"
fi

mkdir -p "$work/bin"
for cmd in docker ufw systemctl chown; do
  printf '#!/bin/sh\necho "%s $*" >> "%s/calls"\n' "$cmd" "$work" > "$work/bin/$cmd"
done
printf '#!/bin/sh\necho "1.1.1.1 via 203.0.113.1 dev eth0 src 203.0.113.10 uid 0"\n' > "$work/bin/ip"
printf '#!/bin/sh\nexit 0\n' > "$work/bin/curl"
chmod +x "$work/bin/"*

run() {
  cp "$work/root/.env.orig" "$work/root/.env"
  : > "$work/calls"
  env PATH="$work/bin:$PATH" ARTCRAFT_ROOT="$work/root" ARTCRAFT_LOG="$work/log" \
    ARTCRAFT_SECRET_FILE="$work/secret" ARTCRAFT_WAIT_SECONDS=0 bash "$work/root/install.sh" > /dev/null
}
cp "$work/root/.env" "$work/root/.env.orig"

# 1. No domain: sslip.io name from the route's source address, generated password, no mail.
run
grep -qx 'ARTCRAFT_DOMAIN=203-0-113-10.sslip.io' "$work/root/.env"
grep -Eq '^ARTCRAFT_ADMIN_PASSWORD=[A-Za-z0-9]{24}$' "$work/root/.env"
grep -q '^203-0-113-10.sslip.io {' "$work/root/Caddyfile"
if grep -q 'email' "$work/root/Caddyfile"; then echo 'unexpected email'; exit 1; fi
grep -q 'password: ' "$work/secret"
grep -q '^docker compose up -d' "$work/calls"
if grep -q 'ufw allow 25' "$work/calls"; then echo 'unexpected mail port'; exit 1; fi

# 2. Domain, ACME mail, fixed password and mail on.
sed -i -e 's/^ARTCRAFT_DOMAIN=.*/ARTCRAFT_DOMAIN=cloud.example.eu/' \
  -e 's/^ARTCRAFT_ACME_EMAIL=.*/ARTCRAFT_ACME_EMAIL=me@example.eu/' \
  -e 's/^ARTCRAFT_ADMIN_PASSWORD=.*/ARTCRAFT_ADMIN_PASSWORD=Secret123/' \
  -e 's/^ARTCRAFT_MAIL=.*/ARTCRAFT_MAIL=1/' "$work/root/.env.orig"
run
grep -qx 'ARTCRAFT_DOMAIN=cloud.example.eu' "$work/root/.env"
grep -qx 'ARTCRAFT_ADMIN_PASSWORD=Secret123' "$work/root/.env"
grep -q 'email me@example.eu' "$work/root/Caddyfile"
grep -q '^cloud.example.eu {' "$work/root/Caddyfile"
grep -q '^docker compose --profile mail up -d' "$work/calls"
grep -q 'ufw allow 25/tcp' "$work/calls"

echo "cloud-init recipe OK"
