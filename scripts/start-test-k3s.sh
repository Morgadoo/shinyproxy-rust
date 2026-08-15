#!/usr/bin/env bash
#
# ShinyProxy
#
# Copyright (C) 2016-2026 Open Analytics
#
# ===========================================================================
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the Apache License as published by
# The Apache Software Foundation, either version 2 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# Apache License for more details.
#
# You should have received a copy of the Apache License
# along with this program.  If not, see <http://www.apache.org/licenses/>
#
# Starts a single node Kubernetes cluster (k3s in Docker) for the Kubernetes tests and imports the
# `sp-testapp:test` image into it:
#
#   ./scripts/build-test-image.sh
#   ./scripts/start-test-k3s.sh
#   SP_TEST_K8S=1 cargo test -p shinyproxy --test kubernetes -- --test-threads=1
#
# The kubeconfig is written to /tmp/k3s/kubeconfig.yaml (which the tests use through KUBECONFIG).

set -euo pipefail

cd "$(dirname "$0")/.."

name="${SP_TEST_K3S_CONTAINER:-test-k3s}"
image="${SP_TEST_IMAGE:-sp-testapp:test}"
output="${SP_TEST_K3S_OUTPUT:-/tmp/k3s}"

docker rm -f "$name" >/dev/null 2>&1 || true
rm -rf "$output"
mkdir -p "$output"

# `--snapshotter=native` and `--flannel-backend=host-gw` keep k3s working in a sandbox where overlayfs and
# VXLAN are not available; the disabled components are not needed by the tests.
docker run -d --name "$name" --privileged -p 6443:6443 -v "$output:/output" \
    rancher/k3s:v1.31.5-k3s1 server \
    --snapshotter=native \
    --flannel-backend=host-gw \
    --disable=traefik,servicelb,metrics-server,coredns,local-storage \
    --disable-network-policy \
    --write-kubeconfig=/output/kubeconfig.yaml \
    --write-kubeconfig-mode=666 >/dev/null

echo "waiting for the node to become ready"
for _ in $(seq 1 90); do
    if docker exec "$name" kubectl get nodes 2>/dev/null | grep -q " Ready "; then
        break
    fi
    sleep 1
done
docker exec "$name" kubectl get nodes

echo "importing $image into the cluster"
docker save "$image" | docker exec -i "$name" ctr -n k8s.io images import - >/dev/null

echo "the cluster is available; use KUBECONFIG=$output/kubeconfig.yaml"
