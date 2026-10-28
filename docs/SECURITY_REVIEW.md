# Security review and feature parity (October 2026)

This review covers the whole Rust implementation (about 42,000 lines) against ShinyProxy 3.2.4 /
ContainerProxy 1.2.4, the Java versions it replaces. It had three parts:

- a feature-parity inventory, made from the Java sources rather than from this repository's own docs;
- a security audit in five areas: authentication and sessions, authorization and web output, the reverse
  proxy, the container backends and expressions, and the supply chain;
- fixes for the confirmed problems, each with a regression test.

Severity uses the usual scale: **Critical** means remote and unauthenticated, or a complete break of user
isolation; **Low** means hardening.

Status after the fixes: `cargo test --workspace --all-targets` passes 569 tests, the Redis suites pass
(`SP_TEST_REDIS=1`), `cargo clippy -D warnings` is clean, and `cargo audit` reports nothing beyond one
accepted, documented advisory.

## Fixed

| # | Severity | Finding | Fix |
| --- | --- | --- | --- |
| 1 | **Critical** | With `authentication: none`, every new visitor was the same user (`anonymous-69b097efd7b4`). The anonymous id was derived from the session id, which does not exist until a new session is first saved. Every visitor could therefore list, use and stop the apps of all the others. | A random UUID per session, as `NoAuthenticationBackend` does. Test: `every_anonymous_visitor_is_a_different_user`, which fails without the fix. |
| 2 | **High** | The `kubernetes-pod-patches`, `kubernetes-additional-(persistent-)manifests` and the `ecs-*` fields (EFS `root-directory`, bind volumes, roles, ...) were never evaluated. A per-user volume such as `path: /exports/#{proxy.userId}` reached the cluster verbatim, so all users shared one directory. An unquoted `subPath: #{...}` (a YAML comment) even mounted the whole volume. | Resolved in the same phases as Java: ECS in the first phase, Kubernetes in the final one. The YAML is parsed first and only its strings are evaluated, so a user value cannot inject YAML. Java evaluates the raw text, so that is still the fallback for the comment form. |
| 3 | **High** | LDAP accepted an empty password. A DN with an empty password is an *unauthenticated bind* (RFC 4513 §5.1.2), which Active Directory and others answer with success, so anyone could log in as anyone. | Empty user names and passwords are refused before binding, as Spring's `BindAuthenticator` does. |
| 4 | **High** (inherited from Java) | `webservice` authentication pasted the credentials into the JSON body unescaped. A password like `x","username":"attacker","password":"pw` made the service check the attacker's account while ShinyProxy logged in the victim. | The values are JSON-escaped. |
| 5 | **Medium** (inherited) | Kubernetes turned *any* environment value starting with `secretKeyRef:` into a secret reference, including `SHINYPROXY_USERNAME`. A user able to choose their name (OIDC `name`/`nickname`, self-registration) could read any Secret of the namespace from their app. | Only a reference written in `container-env` (or in the env file) becomes one. |
| 6 | **Medium** | No header-read timeout. `axum::serve` builds connections without a timer, which silently disables hyper's 30 s default. A single client could exhaust the file descriptors with idle or slowloris connections, without logging in. Applies to the main and the management port. | A serve loop that configures the timer: 30 s to send headers, 30 s keep-alive idle. |
| 7 | **Medium** | In-memory sessions were never removed. `tower_sessions::MemoryStore` only hides expired records, and every cookieless request creates one (with the requested URL). A script could grow the memory without bound (reproduced: +108 MB for 3,000 requests). | `MemorySessionStore` purges expired sessions; the saved post-login URL is limited to 2 KiB. |
| 8 | **Medium** | The Redis password was logged at INFO on every start (`Using the Redis store (redis://:secret@...)`) and repeated in connection errors. It was also not URL-encoded, so passwords with `@ / # ?` broke the URL. | The URL is redacted in logs and errors, and the credentials are percent-encoded. |
| 9 | **Medium** | Issue-report mail always went out without TLS, with the SMTP credentials. `spring.mail.properties` was parsed but ignored. | `mail.smtp.ssl.enable` / `starttls.required` / `starttls.enable` select TLS as with JavaMail. A warning is logged when credentials would be sent unencrypted. |
| 10 | **Medium** | bcrypt ran on the async runtime: 16 concurrent logins stalled every proxied app. Separately, the dummy hash for unknown users was malformed, so unknown users answered in 2 ms against 600 ms for known ones (user enumeration). | bcrypt runs on a blocking thread, and the dummy is a valid hash with the cost of the configured ones. |
| 11 | **Medium** | `spring.data.redis.sentinel.*` was marked *supported* but ignored. A Sentinel configuration connected to `localhost:6379` and sent the password there. | The server refuses to start with a Sentinel configuration, and the schema says *unsupported*. |
| 12 | **Low** | No equivalent of Spring's `StrictHttpFirewall`. `/app_i/01_hello/..%2F..%2Flogout` made the app page POST to `/logout`, and `..` segments escaped an app's `target-path`. | A request firewall with Spring's rules (encoded `/ \ . %`, `;`, `//`, dot segments, control characters), answering 400 like Java's `ErrorController`. The instance name is also validated on `GET /app_i/...`. |
| 13 | **Low** | `custom-header`: the session cookie alone kept a user logged in without the header (that is, without passing the reverse proxy), and a different header silently switched the session's user. | Same as `CustomHeaderAuthenticationFilter`: a request without the header is not authenticated, and a mismatching header is refused. |
| 14 | **Low** | OpenID Connect without `jwks-url` accepted an unverified id token. | Login fails closed, as Spring does (`missing_signature_verifier`). |
| 15 | **Low** | A request that loaded the session before `/logout` could save it again afterwards, logging the user back in. | `save` only updates existing sessions (`SET ... XX` in Redis). |
| 16 | **Low** | The client's `X-SP-UserId`/`X-SP-UserGroups` reached the app when the configured value was invalid, when `add-default-http-headers: false` was set, or when spelled `X_SP_UserId`. | Client copies of the identity headers and of the app's `http-headers` are always removed. |
| 17 | **Low** | SpEL: `Long.MIN_VALUE / -1` and integer overflow panicked; deeply nested expressions overflowed the stack and aborted the process. | Wrapping arithmetic, as in Java, and a nesting limit of 64 in the parser. |
| 18 | **Low** | `management.server.address` was ignored, so an actuator an admin had restricted to `127.0.0.1` in Java was exposed on all interfaces (the Prometheus metrics carry user ids). `/v3/api-docs` and `/swagger-ui` were public, while Java requires a login. | Both now behave as in Java. |
| 19 | Supply chain | 16 RustSec advisories in the lock file, including XSS fixes in `ammonia` (which sanitizes app descriptions) and five `rustls`/`rustls-webpki`/`webpki-roots` TLS issues. The CI `audit` job would fail. | Semver-compatible updates. The workflows now get least-privilege tokens, and the Docker build uses `--locked`. |

### Functional bugs fixed along the way

- **Startup deadline.** Apps had **5 s** to start instead of Java's 20 s. `container-wait-timeout` (per request) was used as the total deadline instead of `container-wait-time`.
- **Injected pages lost their last bytes.** The script injector never flushed its buffered tail, so HTML pages without `<head>`/`</body>` lost their last 7 bytes and never got the script.
- **App cookies lost their lifetime.** The middleware that makes the ShinyProxy cookie a browser-session cookie also stripped `Max-Age`/`Expires` from every cookie the apps set.
- **ECS `#{...}` values reset the extension.** An `ecs-*` boolean or number written as an expression would have reset the whole extension to its defaults. An invalid extension now logs an error instead of silently dropping all pod patches.

## Open findings (recommendations)

None of these is exploitable without an unusual deployment, or Java behaves the same way. They are listed
roughly in order of value.

| Severity | Finding | Recommendation |
| --- | --- | --- |
| Medium (Java same) | The ShinyProxy session cookie and the `Authorization` header are forwarded to the app containers. With re-used pre-initialized containers or apps that run user code, a previous user of a container could capture the next user's session. | Remove the ShinyProxy cookie from the forwarded `Cookie` header, drop `Authorization` when it carries the ShinyProxy bearer token, and remove app `Set-Cookie` headers that use the ShinyProxy cookie name. |
| Low | `X-Forwarded-For/-Proto/-Port` from the client are forwarded verbatim, and `absolute_url` trusts `X-Forwarded-Host` from any client. | Add a trusted-proxy setting (as `server.forward-headers-strategy`) and overwrite the headers otherwise. |
| Low (Java same) | No `Origin` check on WebSocket upgrades. SameSite=Lax protects the default setup, but not `same-site-cookie: None`. | Optional allow-list for `Origin`. |
| Low (Java same) | `max-instances` is check-then-act, so parallel starts can exceed it. | A per-user lock around the check and the insert. |
| Low | No connect or response timeout to the apps, and WebSocket tunnels have no idle limit. | Connect timeout on the `HttpConnector`; optional tunnel idle timeout. |
| Low | `RedisLock` acquire/release is GET-then-EXPIRE/DEL (not atomic), so two leaders can briefly coexist. Ports of recovered containers are registered under the container id but released under the proxy id, so they leak. | Lua compare-and-set; register recovered ports under the proxy id. |
| Info | `/admin/about` shows the process arguments (Java shows the JVM arguments), which include secrets passed as `--spring.data.redis.password=...`. | Redact values of keys that contain `password`/`secret`. |
| Info (Java same) | CSV usage statistics are not protected against formula injection (`=HYPERLINK(...)` user names). | Prefix `= + - @` cells with `'`. |
| Info | The WebSocket tunnel drops client chunks of 6 bytes or fewer that start with `0x8A` (a pong), which can corrupt a frame split at that boundary. Pings are also injected into non-WebSocket upgrades. An app's own 503 is replaced by `app_crashed`. | Parse frame boundaries before dropping, restrict pings to `Upgrade: websocket`, and pass healthy 503s through. |

Checked and fine: ownership checks on every proxied request and WebSocket upgrade (no IDOR across all
API/proxy routes), no SSRF, no request smuggling (hyper), admin route matrix, template autoescaping and
HTML sanitizing, open-redirect protection, OIDC state/nonce/PKCE and JWT validation (algorithm confusion
blocked), LDAP filter/DN escaping, CSRF scope (same as Java), session fixation (the id is rotated on
login), parameter value validation, Docker/Kubernetes argument and label handling, SQL usage statistics
(bound values), and log-file paths.

## Feature parity with ShinyProxy 3.2.4

The implementation is close to a drop-in replacement for the common setups: Docker, Swarm or Kubernetes,
with simple, LDAP or OpenID login, used from the browser. These are the verified differences, by impact:

**Missing**

1. `POST /api/proxy/{specId}`: starting an app through the API, the main way scripts start apps. Requests get 405.
2. HTTP Basic authentication for `simple`/`ldap`/`webservice`. `curl -u user:pass .../api/...` does not work; bearer tokens (`proxy.oauth2.*`) do.
3. Custom templates (`proxy.template-path`) are rendered with MiniJinja. Thymeleaf templates are not detected and render broken, and `{template-path}/assets/**` is not served.
4. Redis Sentinel and TLS (now refused at startup instead of ignored).
5. CORS (`proxy.api-security.cors-allowed-origins`) is parsed but not applied.
6. Nothing happens when a session expires. Java stops the apps with `stop-on-logout`, records the logout and closes the session's WebSockets.
7. `RedisEventBridge`: container logs of apps started on another server are not collected, and `?watch=true` status requests on another server wait for their timeout.
8. OpenID: `logout-url` is not evaluated as an expression (so `id_token_hint=#{oidcUser.idToken.tokenValue}` does not work); `client-authentication-method` and `enforce-https-redirect-uri` are ignored; `oidcUser.idToken` / `getAttribute()` are missing in expressions.
9. `server.servlet.session.timeout` is not read; only `spring.session.timeout` is.
10. Smaller: the MS-Office user-agent filter, Kubernetes pod events (`imagePullTime`/`containerScheduleTime` metrics, warnings in the log), `kubernetes-debug-patches`.

**Different without (previously) being documented**

- The cleanup of Kubernetes additional manifests only deletes Secrets, ConfigMaps, PVCs, Services and ServiceAccounts in the pod's namespace. Java deletes every labelled resource type in all app namespaces.
- `resource-name` (Java's ShinyProxy notation) is only accepted as `container-resource-name`.
- When `management.server.port` equals `proxy.port`, startup fails instead of serving the actuator on the main port.
- After login, any saved HTML page is the redirect target; Java only returns to app pages.

**Documented gaps, confirmed**

SAML, S3 log storage and the issue-mail log attachments, InfluxDB usage statistics, `logging.requestdump`,
`usage-stats-attributes[].expression`, ECS cleanup and real-AWS validation, resuming with new parameters,
`SPRING_APPLICATION_JSON` and multi-document YAML, the Redis session format (not shared with Java),
histogram buckets, and the Redis health check.

The stale rows in `docs/COMPATIBILITY.md` were corrected: Redis sessions, rolling updates and shared
seats are implemented; the actuator is only on the management port; delegate-proxy removal works.
`README.md` no longer claims every endpoint is implemented.
