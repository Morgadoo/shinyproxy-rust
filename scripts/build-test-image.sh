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
# Builds the `sp-testapp:test` image used by the Docker backend tests
# (`SP_TEST_DOCKER=1 cargo test`). Needs a Docker daemon.

set -euo pipefail

cd "$(dirname "$0")/.."

image="${SP_TEST_IMAGE:-sp-testapp:test}"

echo "building sp-testapp"
cargo build -p testapp

context="$(mktemp -d)"
trap 'rm -rf "$context"' EXIT
cp scripts/testapp-image/Dockerfile "$context/Dockerfile"
cp target/debug/sp-testapp "$context/sp-testapp"

echo "building image $image"
docker build --quiet --tag "$image" "$context"
echo "done: $image"
