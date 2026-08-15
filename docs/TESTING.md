# Testing

Everything is tested with `cargo test`; the suites that need a service (Docker, Kubernetes, Redis, LDAP) are
skipped unless the service is there and the corresponding environment variable is set. `cargo test
--workspace` therefore runs on any machine, in a few seconds, without setup.

```sh
cargo test --workspace          # ~500 tests, no external services
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
./scripts/license-header.sh --check
```

The inventory of the suites (what each one covers) is in [PROGRESS.md](PROGRESS.md#test-inventory).

## The layers of the test suite

| Layer | Where | What it covers |
| --- | --- | --- |
| Unit tests | next to the code (`#[cfg(test)]`) | configuration loading and validation, the expression engine, the domain model, the manifests the Kubernetes and ECS backends build, the seat model, the stores, the security helpers |
| Golden files | `crates/containerproxy/tests/golden.rs` | the canonical YAML and the SHA-1 instance id, byte-compared with output captured from Jackson |
| HTML snapshots | `crates/shinyproxy/tests/html_snapshots.rs` (insta) | every page, so a template change is visible in the diff |
| End-to-end (in process) | `crates/shinyproxy/tests/*.rs` | a real server on a real port with a real app process (`sp-testapp`): login, starting apps, proxying HTTP and WebSocket traffic, heartbeats, the API, the admin pages, timers, metrics, parameters, pre-initialized containers |
| End-to-end (with a service) | the suites below | the container backends, Redis, LDAP |

The end-to-end tests use the harness in `crates/shinyproxy/tests/common/mod.rs`: `TestInstance::start(yaml)`
loads a configuration through the same code path as the binary, runs the same startup sequence and serves on
an ephemeral port. Every instance gets its own host port range, so the tests can run in parallel.

## Suites that need a service

### Docker

```sh
sudo dockerd &                        # or the daemon of the machine
./scripts/build-test-image.sh         # builds sp-testapp:test
SP_TEST_DOCKER=1 cargo test -p shinyproxy --test docker -- --test-threads=1
```

Covers the container create request (labels, environment, published ports), HTTP and WebSocket proxying to a
real container, stop and cleanup, pause/resume and app recovery after a restart.

### Kubernetes

```sh
./scripts/build-test-image.sh
./scripts/start-test-k3s.sh           # k3s in Docker, imports sp-testapp:test
SP_TEST_K8S=1 KUBECONFIG=/tmp/k3s/kubeconfig.yaml \
    cargo test -p shinyproxy --test kubernetes -- --test-threads=1
```

Covers the pod and the `NodePort` service that are created, proxying to the pod, cleanup on stop, pod patches
and additional/persistent manifests, and the recovery of running pods.

### Redis (high availability)

```sh
redis-server --port 6379 --save "" --appendonly no &
SP_TEST_REDIS=1 cargo test -p shinyproxy --test redis_store -- --test-threads=1
# the unit tests of the Redis backed services use the same variable
SP_TEST_REDIS=1 cargo test -p containerproxy --lib
```

Covers two servers of one realm: shared apps (start on one, use and stop on the other), shared sessions,
shared heartbeats, host port allocation without collisions, the leader election, rolling updates with
`proxy.version` and shared pre-initialized containers. `SP_TEST_REDIS_URL` overrides the URL.

### LDAP

```sh
./scripts/start-test-ldap.sh          # OpenLDAP in Docker, seeded with jack and jeff
SP_TEST_LDAP=1 cargo test -p shinyproxy --test ldap -- --test-threads=1
```

Covers a user DN pattern, a user search with a manager account, group based access and admin rights, wrong
passwords, unknown users and an unreachable directory.

### OpenID Connect

No service needed: `crates/shinyproxy/tests/openid.rs` runs a fake provider in process (an RSA key, a JWKS
endpoint, a token endpoint with the refresh grant), so the whole flow is tested with real RS256 tokens.

## Load and soak

`scripts/load-test.sh` starts a server with the Docker backend, opens a number of WebSocket connections
through the proxy and hammers the HTTP path at the same time (the load generator is `sp-testapp --load-test`,
so nothing has to be installed). It prints the throughput, the latency distribution, the resident memory of
the server and its startup time, and it fails when the server panicked.

```sh
./scripts/build-test-image.sh
./scripts/load-test.sh                    # this implementation
./scripts/load-test.sh /path/to/shinyproxy-3.2.4-exec.jar   # the Java one, for comparison
SP_LOAD_SECONDS=1800 SP_LOAD_WEBSOCKETS=200 ./scripts/load-test.sh   # the soak run
```

The numbers of the last comparison are in [COMPATIBILITY.md](COMPATIBILITY.md#performance).

## Comparing the whole behaviour with the Java implementation

`scripts/cross-validate.sh` runs one scenario list against the Java ShinyProxy and this implementation and
diffs the answers; see [COMPATIBILITY.md](COMPATIBILITY.md#cross-validation-against-the-java-implementation).

## Cross-validating the expression engine against Spring

`tools/spel-crossvalidate/run.sh` evaluates a corpus of 116 expressions with the real Spring Expression
Language (it needs a JDK and the ContainerProxy jar) and compares the answers with this implementation. The
corpus and the two documented supersets are in `tools/spel-crossvalidate/`.

## Writing new tests

* Prefer an end-to-end test through `TestInstance` over testing a handler directly: it goes through the
  configuration, the router, the security layer and the store, which is where the parity with Java lives.
* Tests that assert a published host port must use `TestInstance::start_sharing_ports(yaml, (start, max))`,
  because the harness overrides the port range otherwise.
* Tests that assert a startup validation use `common::start_and_expect_error(yaml)`.
* A page change belongs in the snapshot tests (`cargo insta review` after changing a template).
* A test that needs a service goes behind its own `SP_TEST_*` variable and prints a skip message, so
  `cargo test --workspace` stays green everywhere.
