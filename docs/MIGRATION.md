# Migrating from the Java ShinyProxy

This build is a drop-in replacement for ShinyProxy 3.2.4: it reads the same `application.yml`, serves the same
routes and pages, and creates containers with the same names, labels and environment variables. In most
deployments the migration is *replace the jar with the binary*. This page lists everything that needs
attention; the full behaviour comparison lives in [COMPATIBILITY.md](COMPATIBILITY.md).

## The short version

```sh
# before
java -jar shinyproxy-3.2.4.jar

# after
./shinyproxy                                     # reads ./application.yml
./shinyproxy --spring.config.location=/etc/shinyproxy/application.yml
```

Everything else stays the same: the port, the context path, the cookies, the container labels
(`openanalytics.eu/sp-*`), the environment variables (`SHINYPROXY_*`), the API, the metrics and the pages.

## Before you switch

1. **Stop the running apps** (or let them be recovered). The container labels are identical, so
   `proxy.recover-running-proxies: true` also recovers containers that were started by the Java
   implementation, as long as the configuration is unchanged (the instance id is the same hash of the same
   configuration). Apps that were started with a *different* configuration need
   `proxy.recover-running-proxies-from-different-config: true`, exactly as in Java.
2. **Users have to log in again.** Sessions are not carried over:
   * in-memory sessions are lost when any server restarts (also true for the Java implementation);
   * Redis sessions are stored as JSON instead of Spring Session's Java serialisation, so the sessions of a
     Java server cannot be read. The cookie name (`SESSION`) and the key namespace are the same, so the old
     keys are simply ignored and expire.
3. **Check the startup output.** Every property that is not understood is reported as a warning, and the
   configurations that the Java implementation refuses are refused here with the same messages. A
   configuration that starts without warnings is fully understood.

## Things that are not supported

| Feature | What happens | What to do |
| --- | --- | --- |
| `proxy.authentication: saml` | The server refuses to start with an explicit message | Use `openid` (most identity providers offer both), or keep the Java implementation for this deployment |
| `proxy.authentication: keycloak` | The server refuses to start with an explicit message | Use `openid` against the Keycloak realm (`proxy.openid.*`); this is what upstream ShinyProxy recommends since 3.0 |
| `proxy.container-log-s3-*` | The setting is accepted but the logs are only written to `proxy.container-log-path` | Ship the log files with any sidecar (for example `aws s3 sync`) |
| InfluxDB usage statistics (`proxy.usage-stats-url: influxdb://...`) | The server refuses the URL | Use the CSV or the SQL collector, or scrape `/actuator/prometheus` |
| `logging.requestdump` | Ignored | Use `logging.level.containerproxy=debug` for the request logging that exists |
| Spring extension points (custom `IContainerBackend`, `AuthenticationBackend`, ... on the classpath) | Not applicable: there is no classpath | Contribute the backend to this repository, or keep the Java implementation |
| ECS (`proxy.container-backend: ecs`) | Implemented, but never validated against a real AWS account | Validate in a test cluster before moving production |

## Differences you may notice

* **`/admin/about`** shows the Rust build information (version, the ShinyProxy version it is compatible with,
  the commit, the build time, the compiler) instead of the JVM information (heap, JVM arguments, ...). The
  memory usage is reported from the operating system.
* **Log format.** The default log lines carry the same information but not the same layout as Logback.
  `proxy.log-as-json: true` produces the Logstash-style JSON the Java implementation produces, which is what
  log pipelines usually parse.
* **`#{...}` expressions** are evaluated by an implementation of the SpEL subset that ShinyProxy uses (see
  [COMPATIBILITY.md](COMPATIBILITY.md#spel)); it is cross-validated against Spring, but exotic expressions
  (reflection, bean references, `T(...)` type references) are refused with an error at startup instead of
  being evaluated.
* **`parameters.template`** is rendered with MiniJinja instead of Thymeleaf. A custom template that uses
  Thymeleaf attributes (`th:each`, `${...}`) is refused at startup with a message that names the constructs it
  found; port it to MiniJinja (`{% for %}`, `{{ }}`) or drop it and use the built-in form.
* **The `local` backend** (`proxy.container-backend: local`) is an addition of this implementation that runs
  apps as local processes. It exists for the test suite and is not meant for production.

## Verifying a migration

1. Start the binary with the production configuration and `--proxy.port=8081`, and compare the startup
   output: the instance id (`ShinyProxy instanceID (hash of config)`) must be the same as the one the Java
   server logs, because it is the same hash of the same configuration.
2. Compare the pages and the API: `curl -s localhost:8081/api/proxyspec` against the Java server, and the
   HTML of `/` and `/app/{spec}`.
3. Start an app, and check the container: `docker inspect` shows the same labels and environment variables.
4. Switch the traffic. Both implementations can even run side by side against the same Redis realm for the
   apps (the documents in Redis have the same shape), but not for the sessions.
