# Configuration reference

ShinyProxy (Rust) reads the same `application.yml` as the Java implementation (version 3.2.4). This page lists every property that is understood, its shape and whether the behaviour behind it is already implemented in this rewrite.

*This file is generated:* run

```
cargo run -q -p shinyproxy --example config-docs > docs/CONFIGURATION.md
```

after changing `crates/containerproxy/src/config/schema.rs` or `crates/shinyproxy/src/config_schema.rs`.

## How configuration is resolved

1. `--key=value` command line arguments
2. environment variables (`PROXY_PORT`, `PROXY_DOCKER_PORT_RANGE_START`, `PROXY_ADMIN_GROUPS_0`, `PROXY_SPECS_0_CONTAINER_IMAGE`, ...)
3. profile specific files (`application-{profile}.yml`) next to the configuration file
4. the configuration file: `application.yml` in the working directory, `--spring.config.location=<file|dir>` or `SPRING_CONFIG_LOCATION`
5. built-in defaults; when no configuration file exists at all, the built-in demo configuration is used (`demo` profile)

Property names are matched leniently: `port-range-start`, `portRangeStart` and `PORT_RANGE_START` are the same property. `${VAR}` and `${other.property:default}` placeholders are resolved against the environment and the configuration itself; placeholders that cannot be resolved are left untouched (so Thymeleaf snippets such as `${parameterDefinitions}` keep working).

## Properties

| Property | Shape | Status |
| --- | --- | --- |
| `logging.file.name` | value | supported |
| `logging.include-application-name` | value | unsupported: Logback specific; the log format of this implementation always names the module |
| `logging.level` | map (free form keys) | supported |
| `logging.requestdump` | value | unsupported: request dumping is not implemented; use logging.level.* instead |
| `management.defaults.metrics.export.enabled` | value | unsupported: the metrics of this implementation are always collected |
| `management.endpoint.health.group.readiness.include` | value | unsupported: the readiness probe of this implementation always reports app recovery |
| `management.endpoint.health.probes.enabled` | value | unsupported: the liveness and readiness probes are always available |
| `management.endpoints.web.exposure.include` | value | unsupported: health, prometheus and recyclable are always exposed |
| `management.health.ldap.enabled` | value | unsupported: the health endpoint does not check the directory |
| `management.health.redis.enabled` | value | unsupported: the health endpoint does not check Redis |
| `management.server.port` | value | supported |
| `proxy.admin-groups` | list of values | supported |
| `proxy.admin-users` | list of values | supported |
| `proxy.allow-transfer-app` | value | supported |
| `proxy.api-security.cors-allowed-origins` | list of values | supported |
| `proxy.api-security.custom-headers[].name` | value | supported |
| `proxy.api-security.custom-headers[].value` | value | supported |
| `proxy.api-security.disable-hsts-header` | value | supported |
| `proxy.api-security.disable-no-sniff-header` | value | supported |
| `proxy.api-security.disable-xss-protection-header` | value | supported |
| `proxy.api-security.hide-spec-details` | value | supported |
| `proxy.authentication` | value | supported |
| `proxy.bind-address` | value | supported |
| `proxy.body-classes` | list of values | supported |
| `proxy.container-backend` | value | supported |
| `proxy.container-log-path` | value | supported |
| `proxy.container-log-s3-access-key` | value | unsupported: S3 log storage is not implemented; ship the log files instead |
| `proxy.container-log-s3-access-secret` | value | unsupported: S3 log storage is not implemented; ship the log files instead |
| `proxy.container-log-s3-endpoint` | value | unsupported: S3 log storage is not implemented; ship the log files instead |
| `proxy.container-log-s3-sse` | value | unsupported: S3 log storage is not implemented; ship the log files instead |
| `proxy.container-wait-time` | value | supported |
| `proxy.container-wait-timeout` | value | supported |
| `proxy.custom-header.groups-header-name` | value | supported |
| `proxy.custom-header.username-header-name` | value | supported |
| `proxy.default-always-switch-instance` | value | supported |
| `proxy.default-app-logo-classes` | value | supported |
| `proxy.default-app-logo-height` | value | supported |
| `proxy.default-app-logo-style` | value | supported |
| `proxy.default-app-logo-url` | value | supported |
| `proxy.default-app-logo-width` | value | supported |
| `proxy.default-cache-headers-mode` | value | supported |
| `proxy.default-max-instances` | value | supported |
| `proxy.default-proxy-max-lifetime` | value | supported |
| `proxy.default-stop-proxy-on-logout` | value | supported |
| `proxy.default-track-app-url` | value | supported |
| `proxy.default-websocket-reconnection-mode` | value | supported |
| `proxy.docker.cert-path` | value | supported |
| `proxy.docker.container-protocol` | value | supported |
| `proxy.docker.default-container-network` | value | supported |
| `proxy.docker.image-pull-policy` | value | supported |
| `proxy.docker.internal-networking` | value | supported |
| `proxy.docker.loki-url` | value | supported |
| `proxy.docker.port-range-max` | value | supported |
| `proxy.docker.port-range-start` | value | supported |
| `proxy.docker.privileged` | value | supported |
| `proxy.docker.service-wait-time` | value | supported |
| `proxy.docker.target-bind-ip` | value | supported |
| `proxy.docker.target-url` | value | supported |
| `proxy.docker.url` | value | supported |
| `proxy.ecs.cloud-watch-group-prefix` | value | supported |
| `proxy.ecs.cloud-watch-region` | value | supported |
| `proxy.ecs.cloud-watch-stream-prefix` | value | supported |
| `proxy.ecs.container-protocol` | value | supported |
| `proxy.ecs.default-repository-credentials-parameter` | value | supported |
| `proxy.ecs.enable-cloud-watch` | value | supported |
| `proxy.ecs.internal-networking` | value | unsupported: ECS tasks are always reached on the private address of their network interface |
| `proxy.ecs.name` | value | supported |
| `proxy.ecs.privileged` | value | supported |
| `proxy.ecs.region` | value | supported |
| `proxy.ecs.security-groups` | list of values | supported |
| `proxy.ecs.service-wait-time` | value | supported |
| `proxy.ecs.subnets` | list of values | supported |
| `proxy.favicon-path` | value | supported |
| `proxy.heartbeat-rate` | value | supported |
| `proxy.heartbeat-timeout` | value | supported |
| `proxy.hide-navbar` | value | supported |
| `proxy.kubernetes.api-version` | value | supported |
| `proxy.kubernetes.app-namespaces` | list of values | supported |
| `proxy.kubernetes.authorized-additional-manifests` | list of values | supported |
| `proxy.kubernetes.authorized-additional-persistent-manifests` | list of values | supported |
| `proxy.kubernetes.authorized-pod-patches` | list of values | supported |
| `proxy.kubernetes.cert-path` | value | supported |
| `proxy.kubernetes.cluster-domain` | value | supported |
| `proxy.kubernetes.container-protocol` | value | supported |
| `proxy.kubernetes.debug-patches` | value | supported |
| `proxy.kubernetes.image-pull-policy` | value | supported |
| `proxy.kubernetes.image-pull-secret` | value | supported |
| `proxy.kubernetes.image-pull-secrets` | list of values | supported |
| `proxy.kubernetes.internal-networking` | value | supported |
| `proxy.kubernetes.namespace` | value | supported |
| `proxy.kubernetes.node-selector` | value | supported |
| `proxy.kubernetes.node-selector` | map (free form keys) | supported |
| `proxy.kubernetes.pod-wait-time` | value | supported |
| `proxy.kubernetes.privileged` | value | supported |
| `proxy.kubernetes.url` | value | supported |
| `proxy.landing-page` | value | supported |
| `proxy.ldap[].group-search-base` | value | supported |
| `proxy.ldap[].group-search-filter` | value | supported |
| `proxy.ldap[].manager-dn` | value | supported |
| `proxy.ldap[].manager-password` | value | supported |
| `proxy.ldap[].starttls` | value | supported |
| `proxy.ldap[].url` | value | supported |
| `proxy.ldap[].user-dn-pattern` | value | supported |
| `proxy.ldap[].user-search-base` | value | supported |
| `proxy.ldap[].user-search-filter` | value | supported |
| `proxy.log-as-json` | value | supported |
| `proxy.logo-height` | value | supported |
| `proxy.logo-style` | value | supported |
| `proxy.logo-url` | value | supported |
| `proxy.logo-width` | value | supported |
| `proxy.max-total-instances` | value | supported |
| `proxy.monitoring.grafana-url` | value | supported |
| `proxy.ms-graph.api-url` | value | supported |
| `proxy.ms-graph.client-id` | value | supported |
| `proxy.ms-graph.client-secret` | value | supported |
| `proxy.ms-graph.scopes` | list of values | supported |
| `proxy.ms-graph.tenant-id` | value | supported |
| `proxy.ms-graph.token-url` | value | supported |
| `proxy.my-apps-mode` | value | supported |
| `proxy.notification-message` | value | supported |
| `proxy.oauth2.jwks-url` | value | supported |
| `proxy.oauth2.resource-id` | value | supported |
| `proxy.oauth2.roles-claim` | value | supported |
| `proxy.oauth2.username-attribute` | value | supported |
| `proxy.openid.auth-url` | value | supported |
| `proxy.openid.client-authentication-method` | value | supported |
| `proxy.openid.client-id` | value | supported |
| `proxy.openid.client-secret` | value | supported |
| `proxy.openid.enforce-https-redirect-uri` | value | supported |
| `proxy.openid.ignore-session-expire` | value | supported |
| `proxy.openid.include-default-scopes` | value | supported |
| `proxy.openid.jwks-signature-algorithm` | value | supported |
| `proxy.openid.jwks-url` | value | supported |
| `proxy.openid.logout-url` | value | supported |
| `proxy.openid.roles-claim` | value | supported |
| `proxy.openid.scopes` | list of values | supported |
| `proxy.openid.token-url` | value | supported |
| `proxy.openid.userinfo-url` | value | supported |
| `proxy.openid.username-attribute` | value | supported |
| `proxy.openid.with-pkce` | value | supported |
| `proxy.port` | value | supported |
| `proxy.realm-id` | value | supported |
| `proxy.recover-running-proxies` | value | supported |
| `proxy.recover-running-proxies-from-different-config` | value | supported |
| `proxy.same-site-cookie` | value | supported |
| `proxy.saml.app-base-url` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.app-entity-id` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.encryption-cert-name` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.encryption-cert-password` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.force-authn` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.idp-metadata-url` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.keystore` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.keystore-password` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.log-attributes` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.logout-method` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.logout-url` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.name-attribute` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.saml.roles-attribute` | value | unsupported: SAML authentication is not implemented; the server refuses to start with it (use openid) |
| `proxy.seat-wait-time` | value | supported |
| `proxy.specs[].access-expression` | value | supported |
| `proxy.specs[].access-groups` | list of values | supported |
| `proxy.specs[].access-strict-expression` | value | supported |
| `proxy.specs[].access-users` | list of values | supported |
| `proxy.specs[].add-default-http-headers` | value | supported |
| `proxy.specs[].additional-port-mappings[].name` | value | supported |
| `proxy.specs[].additional-port-mappings[].port` | value | supported |
| `proxy.specs[].additional-port-mappings[].target-path` | value | supported |
| `proxy.specs[].allow-container-re-use` | value | supported |
| `proxy.specs[].always-show-switch-instance` | value | supported |
| `proxy.specs[].cache-headers-mode` | value | supported |
| `proxy.specs[].container-cmd` | list of values | supported |
| `proxy.specs[].container-cpu-limit` | value | supported |
| `proxy.specs[].container-cpu-request` | value | supported |
| `proxy.specs[].container-dns` | list of values | supported |
| `proxy.specs[].container-env` | map (free form keys) | supported |
| `proxy.specs[].container-env-file` | value | supported |
| `proxy.specs[].container-image` | value | supported |
| `proxy.specs[].container-memory-limit` | value | supported |
| `proxy.specs[].container-memory-request` | value | supported |
| `proxy.specs[].container-network` | value | supported |
| `proxy.specs[].container-network-connections` | list of values | supported |
| `proxy.specs[].container-privileged` | value | supported |
| `proxy.specs[].container-resource-name` | value | supported |
| `proxy.specs[].container-volumes` | list of values | supported |
| `proxy.specs[].custom-app-details[].description` | value | supported |
| `proxy.specs[].custom-app-details[].name` | value | supported |
| `proxy.specs[].custom-app-details[].value` | value | supported |
| `proxy.specs[].description` | value | supported |
| `proxy.specs[].display-name` | value | supported |
| `proxy.specs[].docker-device-requests[].capabilities` | list of values | supported |
| `proxy.specs[].docker-device-requests[].count` | value | supported |
| `proxy.specs[].docker-device-requests[].device-ids` | list of values | supported |
| `proxy.specs[].docker-device-requests[].driver` | value | supported |
| `proxy.specs[].docker-device-requests[].options` | map (free form keys) | supported |
| `proxy.specs[].docker-group-add` | list of values | supported |
| `proxy.specs[].docker-ipc` | value | supported |
| `proxy.specs[].docker-registry-domain` | value | supported |
| `proxy.specs[].docker-registry-password` | value | supported |
| `proxy.specs[].docker-registry-username` | value | supported |
| `proxy.specs[].docker-runtime` | value | supported |
| `proxy.specs[].docker-swarm-secrets[].gid` | value | supported |
| `proxy.specs[].docker-swarm-secrets[].mode` | value | supported |
| `proxy.specs[].docker-swarm-secrets[].name` | value | supported |
| `proxy.specs[].docker-swarm-secrets[].target` | value | supported |
| `proxy.specs[].docker-swarm-secrets[].uid` | value | supported |
| `proxy.specs[].docker-user` | value | supported |
| `proxy.specs[].ecs-bind-volumes` | list of values | supported |
| `proxy.specs[].ecs-cpu-architecture` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].access-point-id` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].enable-iam` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].file-system-id` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].name` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].root-directory` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].transit-encryption` | value | supported |
| `proxy.specs[].ecs-efs-volumes[].transit-encryption-port` | value | supported |
| `proxy.specs[].ecs-enable-execute-command` | value | supported |
| `proxy.specs[].ecs-ephemeral-storage-size` | value | supported |
| `proxy.specs[].ecs-execution-role` | value | supported |
| `proxy.specs[].ecs-managed-secrets[].name` | value | supported |
| `proxy.specs[].ecs-managed-secrets[].value-from` | value | supported |
| `proxy.specs[].ecs-operation-system-family` | value | supported |
| `proxy.specs[].ecs-readonly-root-filesystem` | value | supported |
| `proxy.specs[].ecs-repository-credentials-parameter` | value | supported |
| `proxy.specs[].ecs-task-role` | value | supported |
| `proxy.specs[].external-url` | value | supported |
| `proxy.specs[].favicon-path` | value | supported |
| `proxy.specs[].heartbeat-timeout` | value | supported |
| `proxy.specs[].hide-navbar-on-main-page-link` | value | supported |
| `proxy.specs[].http-headers` | map (free form keys) | supported |
| `proxy.specs[].id` | value | supported |
| `proxy.specs[].kubernetes-additional-manifests` | list of values | supported |
| `proxy.specs[].kubernetes-additional-persistent-manifests` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-additional-manifests[].access-control.groups` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-additional-manifests[].manifests` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].access-control.groups` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].manifests` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.expression` | value | supported |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.groups` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.users` | list of values | supported |
| `proxy.specs[].kubernetes-authorized-pod-patches[].patches` | value | supported |
| `proxy.specs[].kubernetes-pod-patches` | value | supported |
| `proxy.specs[].labels` | map (free form keys) | supported |
| `proxy.specs[].logo-classes` | value | supported |
| `proxy.specs[].logo-height` | value | supported |
| `proxy.specs[].logo-style` | value | supported |
| `proxy.specs[].logo-url` | value | supported |
| `proxy.specs[].logo-width` | value | supported |
| `proxy.specs[].max-instances` | value | supported |
| `proxy.specs[].max-lifetime` | value | supported |
| `proxy.specs[].max-total-instances` | value | supported |
| `proxy.specs[].minimum-seats-available` | value | supported |
| `proxy.specs[].parameters.definitions[].default-value` | value | supported |
| `proxy.specs[].parameters.definitions[].description` | value | supported |
| `proxy.specs[].parameters.definitions[].display-name` | value | supported |
| `proxy.specs[].parameters.definitions[].id` | value | supported |
| `proxy.specs[].parameters.definitions[].value-names[].name` | value | supported |
| `proxy.specs[].parameters.definitions[].value-names[].value` | value | supported |
| `proxy.specs[].parameters.template` | value | supported |
| `proxy.specs[].parameters.value-sets[].access-control.expression` | value | supported |
| `proxy.specs[].parameters.value-sets[].access-control.groups` | list of values | supported |
| `proxy.specs[].parameters.value-sets[].access-control.users` | list of values | supported |
| `proxy.specs[].parameters.value-sets[].name` | value | supported |
| `proxy.specs[].parameters.value-sets[].values` | map (free form keys) | supported |
| `proxy.specs[].port` | value | supported |
| `proxy.specs[].scale-down-delay` | value | supported |
| `proxy.specs[].seats-per-container` | value | supported |
| `proxy.specs[].shiny-force-full-reload` | value | supported |
| `proxy.specs[].stop-on-logout` | value | supported |
| `proxy.specs[].support-mail-subject` | value | supported |
| `proxy.specs[].support-mail-to-address` | value | supported |
| `proxy.specs[].target-path` | value | supported |
| `proxy.specs[].template-group` | value | supported |
| `proxy.specs[].template-properties` | map (free form keys) | supported |
| `proxy.specs[].track-app-url` | value | supported |
| `proxy.specs[].websocket-reconnection-mode` | value | supported |
| `proxy.stop-proxies-on-shutdown` | value | supported |
| `proxy.store-mode` | value | supported |
| `proxy.support.mail-from-address` | value | supported |
| `proxy.support.mail-subject` | value | supported |
| `proxy.support.mail-to-address` | value | supported |
| `proxy.template-groups[].id` | value | supported |
| `proxy.template-groups[].properties` | map (free form keys) | supported |
| `proxy.template-path` | value | supported |
| `proxy.title` | value | supported |
| `proxy.usage-stats-attributes[].expression` | value | supported |
| `proxy.usage-stats-attributes[].name` | value | supported |
| `proxy.usage-stats-hikari.connection-timeout` | value | supported |
| `proxy.usage-stats-hikari.idle-timeout` | value | supported |
| `proxy.usage-stats-hikari.max-lifetime` | value | supported |
| `proxy.usage-stats-hikari.maximum-pool-size` | value | supported |
| `proxy.usage-stats-hikari.minimum-idle` | value | supported |
| `proxy.usage-stats-micrometer-prefix` | value | supported |
| `proxy.usage-stats-password` | value | supported |
| `proxy.usage-stats-table-name` | value | supported |
| `proxy.usage-stats-url` | value | supported |
| `proxy.usage-stats-username` | value | supported |
| `proxy.usage-stats[].attributes[].expression` | value | supported |
| `proxy.usage-stats[].attributes[].name` | value | supported |
| `proxy.usage-stats[].password` | value | supported |
| `proxy.usage-stats[].table-name` | value | supported |
| `proxy.usage-stats[].url` | value | supported |
| `proxy.usage-stats[].username` | value | supported |
| `proxy.username-case-sensitive` | value | supported |
| `proxy.users[].groups` | list of values | supported |
| `proxy.users[].name` | value | supported |
| `proxy.users[].password` | value | supported |
| `proxy.version` | value | supported |
| `proxy.webservice.authentication-request-body` | value | supported |
| `proxy.webservice.authentication-url` | value | supported |
| `proxy.webservice.groups-expression` | value | supported |
| `server.frame-options` | value | supported |
| `server.secure-cookies` | value | supported |
| `server.servlet.context-path` | value | supported |
| `server.undertow.max-http-post-size` | value | unsupported: Undertow specific |
| `server.use-forward-headers` | value | unsupported: removed in ShinyProxy 3.x |
| `spring.application.name` | value | supported |
| `spring.config.location` | value | supported |
| `spring.data.redis.database` | value | supported |
| `spring.data.redis.host` | value | supported |
| `spring.data.redis.password` | value | supported |
| `spring.data.redis.port` | value | supported |
| `spring.data.redis.sentinel.master` | value | supported |
| `spring.data.redis.sentinel.nodes` | list of values | supported |
| `spring.data.redis.sentinel.password` | value | supported |
| `spring.data.redis.username` | value | supported |
| `spring.mail.host` | value | supported |
| `spring.mail.password` | value | supported |
| `spring.mail.port` | value | supported |
| `spring.mail.properties` | map (free form keys) | supported |
| `spring.mail.username` | value | supported |
| `spring.profiles.active` | value | supported |
| `spring.servlet.multipart.enabled` | value | unsupported: bodies are never buffered |
| `spring.session.redis.flush-mode` | value | unsupported: Spring Session specific; sessions are written immediately |
| `spring.session.redis.repository-type` | value | unsupported: Spring Session specific; there is one Redis session store |
| `spring.session.store-type` | value | supported |
| `spring.session.timeout` | value | supported |
| `springdoc.api-docs.enabled` | value | supported |
| `springdoc.swagger-ui.enabled` | value | supported |

## Notes

* `proxy.ldap` accepts a single provider (`proxy.ldap.url`) or a list of providers (`proxy.ldap[0].url`).
* `proxy.ecs.enable-cloudwatch` is accepted as an alias of `proxy.ecs.enable-cloud-watch`.
* `proxy.container-backend: local` is an addition of this implementation: it starts apps as local processes and exists for testing only.
* Deviations from the Java implementation are tracked in [COMPATIBILITY.md](COMPATIBILITY.md).
