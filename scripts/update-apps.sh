#!/bin/sh
# Update, roll back or list the ArtCraft Crafting Apps installed in the data volume.
#
#   update-apps                 # fetch the latest release of every app (SHA-256 + signature checked)
#   update-apps wordcraft       # only one app
#   update-apps --rollback ID   # switch ID back to the version kept from the previous update
#   update-apps --list          # what is installed
#
# Outside the container it runs the same command through docker/podman or kubectl:
#   ARTCRAFT_CONTAINER=artcraft-cloud update-apps          (docker compose / docker run)
#   ARTCRAFT_K8S=deploy/artcraft-cloud update-apps         (Kubernetes, current namespace)
# The running server picks up the new versions on the next request; no restart needed.
set -eu

case "${1:-}" in
  --list|list) set -- apps list ;;
  --rollback|rollback) [ $# -ge 2 ] || { echo "usage: $0 --rollback APP" >&2; exit 2; }; set -- apps rollback "$2" ;;
  -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
  "") set -- apps update ;;
  *) set -- apps update "$1" ;;
esac

if [ -n "${ARTCRAFT_K8S:-}" ]; then
  exec kubectl exec -i "$ARTCRAFT_K8S" -- artcraft-cloud "$@"
elif [ -n "${ARTCRAFT_CONTAINER:-}" ]; then
  engine="${CONTAINER_ENGINE:-$(command -v docker || command -v podman)}"
  exec "$engine" exec -i "$ARTCRAFT_CONTAINER" artcraft-cloud "$@"
else
  exec artcraft-cloud "$@"
fi
