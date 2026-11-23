#!/bin/sh
# Re-render deploy/kubernetes/artcraft-cloud.yaml from the Helm chart. Needs helm.
set -eu
cd "$(dirname "$0")/.."
out=deploy/kubernetes/artcraft-cloud.yaml
head="$(sed -n '1,/^---$/p' "$out" | sed '$d')"
tail="$(sed -n '/^# ---$/,$p' "$out")"
{
  printf '%s\n---\napiVersion: v1\nkind: Namespace\nmetadata:\n  name: artcraft-cloud\n' "$head"
  helm template artcraft-cloud deploy/helm/artcraft-cloud --namespace artcraft-cloud \
    | grep -v -E '^# Source:|helm.sh/chart:|app.kubernetes.io/managed-by:' \
    | sed 's/^metadata:$/metadata:\n  namespace: artcraft-cloud/'
  printf '%s\n' "$tail"
} > "$out.tmp"
mv "$out.tmp" "$out"
