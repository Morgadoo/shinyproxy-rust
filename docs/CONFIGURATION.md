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
| `logging.file.name` | value | planned (P10) |
| `logging.include-application-name` | value | planned (P10) |
| `logging.level` | map (free form keys) | planned (P10) |
| `logging.requestdump` | value | planned (P10) |
| `management.defaults.metrics.export.enabled` | value | planned (P10) |
| `management.endpoint.health.group.readiness.include` | value | planned (P10) |
| `management.endpoint.health.probes.enabled` | value | planned (P10) |
| `management.endpoints.web.exposure.include` | value | planned (P10) |
| `management.health.ldap.enabled` | value | planned (P10) |
| `management.health.redis.enabled` | value | planned (P10) |
| `management.server.port` | value | planned (P10) |
| `proxy.admin-groups` | list of values | planned (P4) |
| `proxy.admin-users` | list of values | planned (P4) |
| `proxy.allow-transfer-app` | value | planned (P7) |
| `proxy.api-security.cors-allowed-origins` | list of values | planned (P4) |
| `proxy.api-security.custom-headers[].name` | value | planned (P4) |
| `proxy.api-security.custom-headers[].value` | value | planned (P4) |
| `proxy.api-security.disable-hsts-header` | value | planned (P4) |
| `proxy.api-security.disable-no-sniff-header` | value | planned (P4) |
| `proxy.api-security.disable-xss-protection-header` | value | planned (P4) |
| `proxy.api-security.hide-spec-details` | value | planned (P7) |
| `proxy.authentication` | value | planned (P4) |
| `proxy.bind-address` | value | supported |
| `proxy.body-classes` | list of values | planned (P4) |
| `proxy.container-backend` | value | planned (P5) |
| `proxy.container-log-path` | value | planned (P10) |
| `proxy.container-log-s3-access-key` | value | planned (P10) |
| `proxy.container-log-s3-access-secret` | value | planned (P10) |
| `proxy.container-log-s3-endpoint` | value | planned (P10) |
| `proxy.container-log-s3-sse` | value | planned (P10) |
| `proxy.container-wait-time` | value | planned (P5) |
| `proxy.container-wait-timeout` | value | planned (P5) |
| `proxy.custom-header.groups-header-name` | value | planned (P11) |
| `proxy.custom-header.username-header-name` | value | planned (P11) |
| `proxy.default-always-switch-instance` | value | planned (P9) |
| `proxy.default-app-logo-classes` | value | planned (P4) |
| `proxy.default-app-logo-height` | value | planned (P4) |
| `proxy.default-app-logo-style` | value | planned (P4) |
| `proxy.default-app-logo-url` | value | planned (P4) |
| `proxy.default-app-logo-width` | value | planned (P4) |
| `proxy.default-cache-headers-mode` | value | planned (P6) |
| `proxy.default-max-instances` | value | planned (P5) |
| `proxy.default-proxy-max-lifetime` | value | planned (P5) |
| `proxy.default-stop-proxy-on-logout` | value | planned (P10) |
| `proxy.default-track-app-url` | value | planned (P9) |
| `proxy.default-websocket-reconnection-mode` | value | planned (P6) |
| `proxy.docker.cert-path` | value | planned (P8) |
| `proxy.docker.container-protocol` | value | planned (P8) |
| `proxy.docker.default-container-network` | value | planned (P8) |
| `proxy.docker.image-pull-policy` | value | planned (P8) |
| `proxy.docker.internal-networking` | value | planned (P8) |
| `proxy.docker.loki-url` | value | planned (P8) |
| `proxy.docker.port-range-max` | value | planned (P5) |
| `proxy.docker.port-range-start` | value | planned (P5) |
| `proxy.docker.privileged` | value | planned (P8) |
| `proxy.docker.service-wait-time` | value | planned (P8) |
| `proxy.docker.target-bind-ip` | value | planned (P8) |
| `proxy.docker.target-url` | value | planned (P8) |
| `proxy.docker.url` | value | planned (P8) |
| `proxy.ecs.cloud-watch-group-prefix` | value | planned (P12) |
| `proxy.ecs.cloud-watch-region` | value | planned (P12) |
| `proxy.ecs.cloud-watch-stream-prefix` | value | planned (P12) |
| `proxy.ecs.default-repository-credentials-parameter` | value | planned (P12) |
| `proxy.ecs.enable-cloud-watch` | value | planned (P12) |
| `proxy.ecs.internal-networking` | value | planned (P12) |
| `proxy.ecs.name` | value | planned (P12) |
| `proxy.ecs.privileged` | value | planned (P12) |
| `proxy.ecs.region` | value | planned (P12) |
| `proxy.ecs.security-groups` | list of values | planned (P12) |
| `proxy.ecs.service-wait-time` | value | planned (P12) |
| `proxy.ecs.subnets` | list of values | planned (P12) |
| `proxy.favicon-path` | value | planned (P4) |
| `proxy.heartbeat-rate` | value | planned (P6) |
| `proxy.heartbeat-timeout` | value | planned (P6) |
| `proxy.hide-navbar` | value | planned (P4) |
| `proxy.kubernetes.api-version` | value | planned (P12) |
| `proxy.kubernetes.authorized-additional-manifests` | list of values | planned (P12) |
| `proxy.kubernetes.authorized-additional-persistent-manifests` | list of values | planned (P12) |
| `proxy.kubernetes.authorized-pod-patches` | list of values | planned (P12) |
| `proxy.kubernetes.cert-path` | value | planned (P12) |
| `proxy.kubernetes.cluster-domain` | value | planned (P12) |
| `proxy.kubernetes.container-protocol` | value | planned (P12) |
| `proxy.kubernetes.debug-patches` | value | planned (P12) |
| `proxy.kubernetes.image-pull-policy` | value | planned (P12) |
| `proxy.kubernetes.image-pull-secret` | value | planned (P12) |
| `proxy.kubernetes.image-pull-secrets` | list of values | planned (P12) |
| `proxy.kubernetes.internal-networking` | value | planned (P12) |
| `proxy.kubernetes.namespace` | value | planned (P12) |
| `proxy.kubernetes.node-selector` | map (free form keys) | planned (P12) |
| `proxy.kubernetes.pod-wait-time` | value | planned (P12) |
| `proxy.kubernetes.privileged` | value | planned (P12) |
| `proxy.kubernetes.url` | value | planned (P12) |
| `proxy.landing-page` | value | planned (P4) |
| `proxy.ldap[].group-search-base` | value | planned (P11) |
| `proxy.ldap[].group-search-filter` | value | planned (P11) |
| `proxy.ldap[].manager-dn` | value | planned (P11) |
| `proxy.ldap[].manager-password` | value | planned (P11) |
| `proxy.ldap[].starttls` | value | planned (P11) |
| `proxy.ldap[].url` | value | planned (P11) |
| `proxy.ldap[].user-dn-pattern` | value | planned (P11) |
| `proxy.ldap[].user-search-base` | value | planned (P11) |
| `proxy.ldap[].user-search-filter` | value | planned (P11) |
| `proxy.log-as-json` | value | planned (P10) |
| `proxy.logo-height` | value | planned (P4) |
| `proxy.logo-style` | value | planned (P4) |
| `proxy.logo-url` | value | planned (P4) |
| `proxy.logo-width` | value | planned (P4) |
| `proxy.max-total-instances` | value | planned (P5) |
| `proxy.monitoring.grafana-url` | value | planned (P9) |
| `proxy.ms-graph.api-url` | value | planned (P11) |
| `proxy.ms-graph.client-id` | value | planned (P11) |
| `proxy.ms-graph.client-secret` | value | planned (P11) |
| `proxy.ms-graph.scopes` | list of values | planned (P11) |
| `proxy.ms-graph.tenant-id` | value | planned (P11) |
| `proxy.ms-graph.token-url` | value | planned (P11) |
| `proxy.my-apps-mode` | value | planned (P9) |
| `proxy.notification-message` | value | planned (P4) |
| `proxy.oauth2.jwks-url` | value | planned (P11) |
| `proxy.oauth2.resource-id` | value | planned (P11) |
| `proxy.oauth2.roles-claim` | value | planned (P11) |
| `proxy.oauth2.username-attribute` | value | planned (P11) |
| `proxy.openid.auth-url` | value | planned (P11) |
| `proxy.openid.client-authentication-method` | value | planned (P11) |
| `proxy.openid.client-id` | value | planned (P11) |
| `proxy.openid.client-secret` | value | planned (P11) |
| `proxy.openid.enforce-https-redirect-uri` | value | planned (P11) |
| `proxy.openid.ignore-session-expire` | value | planned (P11) |
| `proxy.openid.include-default-scopes` | value | planned (P11) |
| `proxy.openid.jwks-signature-algorithm` | value | planned (P11) |
| `proxy.openid.jwks-url` | value | planned (P11) |
| `proxy.openid.logout-url` | value | planned (P11) |
| `proxy.openid.roles-claim` | value | planned (P11) |
| `proxy.openid.scopes` | list of values | planned (P11) |
| `proxy.openid.token-url` | value | planned (P11) |
| `proxy.openid.userinfo-url` | value | planned (P11) |
| `proxy.openid.username-attribute` | value | planned (P11) |
| `proxy.openid.with-pkce` | value | planned (P11) |
| `proxy.port` | value | supported |
| `proxy.realm-id` | value | supported |
| `proxy.recover-running-proxies` | value | planned (P8) |
| `proxy.recover-running-proxies-from-different-config` | value | planned (P8) |
| `proxy.same-site-cookie` | value | planned (P4) |
| `proxy.saml.app-base-url` | value | planned (P11) |
| `proxy.saml.app-entity-id` | value | planned (P11) |
| `proxy.saml.encryption-cert-name` | value | planned (P11) |
| `proxy.saml.encryption-cert-password` | value | planned (P11) |
| `proxy.saml.force-authn` | value | planned (P11) |
| `proxy.saml.idp-metadata-url` | value | planned (P11) |
| `proxy.saml.keystore` | value | planned (P11) |
| `proxy.saml.keystore-password` | value | planned (P11) |
| `proxy.saml.log-attributes` | value | planned (P11) |
| `proxy.saml.logout-method` | value | planned (P11) |
| `proxy.saml.logout-url` | value | planned (P11) |
| `proxy.saml.name-attribute` | value | planned (P11) |
| `proxy.saml.roles-attribute` | value | planned (P11) |
| `proxy.seat-wait-time` | value | planned (P12) |
| `proxy.specs[].access-expression` | value | planned (P4) |
| `proxy.specs[].access-groups` | list of values | planned (P2) |
| `proxy.specs[].access-strict-expression` | value | planned (P4) |
| `proxy.specs[].access-users` | list of values | planned (P2) |
| `proxy.specs[].add-default-http-headers` | value | planned (P6) |
| `proxy.specs[].additional-port-mappings[].name` | value | planned (P2) |
| `proxy.specs[].additional-port-mappings[].port` | value | planned (P2) |
| `proxy.specs[].additional-port-mappings[].target-path` | value | planned (P2) |
| `proxy.specs[].allow-container-re-use` | value | planned (P12) |
| `proxy.specs[].always-show-switch-instance` | value | planned (P9) |
| `proxy.specs[].cache-headers-mode` | value | planned (P6) |
| `proxy.specs[].container-cmd` | list of values | planned (P2) |
| `proxy.specs[].container-cpu-limit` | value | planned (P2) |
| `proxy.specs[].container-cpu-request` | value | planned (P2) |
| `proxy.specs[].container-dns` | list of values | planned (P2) |
| `proxy.specs[].container-env` | map (free form keys) | planned (P2) |
| `proxy.specs[].container-env-file` | value | planned (P2) |
| `proxy.specs[].container-image` | value | planned (P2) |
| `proxy.specs[].container-memory-limit` | value | planned (P2) |
| `proxy.specs[].container-memory-request` | value | planned (P2) |
| `proxy.specs[].container-network` | value | planned (P2) |
| `proxy.specs[].container-network-connections` | list of values | planned (P2) |
| `proxy.specs[].container-privileged` | value | planned (P2) |
| `proxy.specs[].container-resource-name` | value | planned (P2) |
| `proxy.specs[].container-volumes` | list of values | planned (P2) |
| `proxy.specs[].custom-app-details[].description` | value | planned (P9) |
| `proxy.specs[].custom-app-details[].name` | value | planned (P9) |
| `proxy.specs[].custom-app-details[].value` | value | planned (P9) |
| `proxy.specs[].description` | value | planned (P2) |
| `proxy.specs[].display-name` | value | planned (P2) |
| `proxy.specs[].docker-device-requests[].capabilities` | list of values | planned (P8) |
| `proxy.specs[].docker-device-requests[].count` | value | planned (P8) |
| `proxy.specs[].docker-device-requests[].device-ids` | list of values | planned (P8) |
| `proxy.specs[].docker-device-requests[].driver` | value | planned (P8) |
| `proxy.specs[].docker-device-requests[].options` | map (free form keys) | planned (P8) |
| `proxy.specs[].docker-group-add` | list of values | planned (P8) |
| `proxy.specs[].docker-ipc` | value | planned (P8) |
| `proxy.specs[].docker-registry-domain` | value | planned (P8) |
| `proxy.specs[].docker-registry-password` | value | planned (P8) |
| `proxy.specs[].docker-registry-username` | value | planned (P8) |
| `proxy.specs[].docker-runtime` | value | planned (P8) |
| `proxy.specs[].docker-swarm-secrets[].gid` | value | planned (P8) |
| `proxy.specs[].docker-swarm-secrets[].mode` | value | planned (P8) |
| `proxy.specs[].docker-swarm-secrets[].name` | value | planned (P8) |
| `proxy.specs[].docker-swarm-secrets[].target` | value | planned (P8) |
| `proxy.specs[].docker-swarm-secrets[].uid` | value | planned (P8) |
| `proxy.specs[].docker-user` | value | planned (P8) |
| `proxy.specs[].ecs-bind-volumes` | list of values | planned (P12) |
| `proxy.specs[].ecs-cpu-architecture` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].access-point-id` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].enable-iam` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].file-system-id` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].name` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].root-directory` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].transit-encryption` | value | planned (P12) |
| `proxy.specs[].ecs-efs-volumes[].transit-encryption-port` | value | planned (P12) |
| `proxy.specs[].ecs-enable-execute-command` | value | planned (P12) |
| `proxy.specs[].ecs-ephemeral-storage-size` | value | planned (P12) |
| `proxy.specs[].ecs-execution-role` | value | planned (P12) |
| `proxy.specs[].ecs-managed-secrets[].name` | value | planned (P12) |
| `proxy.specs[].ecs-managed-secrets[].value-from` | value | planned (P12) |
| `proxy.specs[].ecs-operation-system-family` | value | planned (P12) |
| `proxy.specs[].ecs-readonly-root-filesystem` | value | planned (P12) |
| `proxy.specs[].ecs-repository-credentials-parameter` | value | planned (P12) |
| `proxy.specs[].ecs-task-role` | value | planned (P12) |
| `proxy.specs[].external-url` | value | planned (P9) |
| `proxy.specs[].favicon-path` | value | planned (P2) |
| `proxy.specs[].heartbeat-timeout` | value | planned (P6) |
| `proxy.specs[].hide-navbar-on-main-page-link` | value | planned (P9) |
| `proxy.specs[].http-headers` | map (free form keys) | planned (P6) |
| `proxy.specs[].id` | value | planned (P2) |
| `proxy.specs[].kubernetes-additional-manifests` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-additional-persistent-manifests` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-additional-manifests[].access-control.groups` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-additional-manifests[].manifests` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].access-control.groups` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-additional-persistent-manifests[].manifests` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.expression` | value | planned (P12) |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.groups` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-pod-patches[].access-control.users` | list of values | planned (P12) |
| `proxy.specs[].kubernetes-authorized-pod-patches[].patches` | value | planned (P12) |
| `proxy.specs[].kubernetes-pod-patches` | value | planned (P12) |
| `proxy.specs[].labels` | map (free form keys) | planned (P2) |
| `proxy.specs[].logo-classes` | value | planned (P2) |
| `proxy.specs[].logo-height` | value | planned (P2) |
| `proxy.specs[].logo-style` | value | planned (P2) |
| `proxy.specs[].logo-url` | value | planned (P2) |
| `proxy.specs[].logo-width` | value | planned (P2) |
| `proxy.specs[].max-instances` | value | planned (P5) |
| `proxy.specs[].max-lifetime` | value | planned (P5) |
| `proxy.specs[].max-total-instances` | value | planned (P5) |
| `proxy.specs[].minimum-seats-available` | value | planned (P12) |
| `proxy.specs[].parameters.definitions[].default-value` | value | planned (P9) |
| `proxy.specs[].parameters.definitions[].description` | value | planned (P9) |
| `proxy.specs[].parameters.definitions[].display-name` | value | planned (P9) |
| `proxy.specs[].parameters.definitions[].id` | value | planned (P9) |
| `proxy.specs[].parameters.definitions[].value-names[].name` | value | planned (P9) |
| `proxy.specs[].parameters.definitions[].value-names[].value` | value | planned (P9) |
| `proxy.specs[].parameters.template` | value | planned (P9) |
| `proxy.specs[].parameters.value-sets[].access-control.expression` | value | planned (P9) |
| `proxy.specs[].parameters.value-sets[].access-control.groups` | list of values | planned (P9) |
| `proxy.specs[].parameters.value-sets[].access-control.users` | list of values | planned (P9) |
| `proxy.specs[].parameters.value-sets[].name` | value | planned (P9) |
| `proxy.specs[].parameters.value-sets[].values` | map (free form keys) | planned (P9) |
| `proxy.specs[].port` | value | planned (P2) |
| `proxy.specs[].scale-down-delay` | value | planned (P12) |
| `proxy.specs[].seats-per-container` | value | planned (P12) |
| `proxy.specs[].shiny-force-full-reload` | value | planned (P9) |
| `proxy.specs[].stop-on-logout` | value | planned (P10) |
| `proxy.specs[].support-mail-subject` | value | planned (P9) |
| `proxy.specs[].support-mail-to-address` | value | planned (P9) |
| `proxy.specs[].target-path` | value | planned (P2) |
| `proxy.specs[].template-group` | value | planned (P9) |
| `proxy.specs[].template-properties` | map (free form keys) | planned (P9) |
| `proxy.specs[].track-app-url` | value | planned (P9) |
| `proxy.specs[].websocket-reconnection-mode` | value | planned (P6) |
| `proxy.stop-proxies-on-shutdown` | value | planned (P5) |
| `proxy.store-mode` | value | planned (P12) |
| `proxy.support.mail-from-address` | value | planned (P7) |
| `proxy.support.mail-subject` | value | planned (P7) |
| `proxy.support.mail-to-address` | value | planned (P7) |
| `proxy.template-groups[].id` | value | planned (P9) |
| `proxy.template-groups[].properties` | map (free form keys) | planned (P9) |
| `proxy.template-path` | value | planned (P9) |
| `proxy.title` | value | planned (P4) |
| `proxy.usage-stats-attributes[].expression` | value | planned (P10) |
| `proxy.usage-stats-attributes[].name` | value | planned (P10) |
| `proxy.usage-stats-hikari.connection-timeout` | value | planned (P10) |
| `proxy.usage-stats-hikari.idle-timeout` | value | planned (P10) |
| `proxy.usage-stats-hikari.max-lifetime` | value | planned (P10) |
| `proxy.usage-stats-hikari.maximum-pool-size` | value | planned (P10) |
| `proxy.usage-stats-hikari.minimum-idle` | value | planned (P10) |
| `proxy.usage-stats-micrometer-prefix` | value | planned (P10) |
| `proxy.usage-stats-password` | value | planned (P10) |
| `proxy.usage-stats-table-name` | value | planned (P10) |
| `proxy.usage-stats-url` | value | planned (P10) |
| `proxy.usage-stats-username` | value | planned (P10) |
| `proxy.usage-stats[].attributes[].expression` | value | planned (P10) |
| `proxy.usage-stats[].attributes[].name` | value | planned (P10) |
| `proxy.usage-stats[].password` | value | planned (P10) |
| `proxy.usage-stats[].table-name` | value | planned (P10) |
| `proxy.usage-stats[].url` | value | planned (P10) |
| `proxy.usage-stats[].username` | value | planned (P10) |
| `proxy.username-case-sensitive` | value | planned (P4) |
| `proxy.users[].groups` | list of values | planned (P4) |
| `proxy.users[].name` | value | planned (P4) |
| `proxy.users[].password` | value | planned (P4) |
| `proxy.version` | value | supported |
| `proxy.webservice.authentication-request-body` | value | planned (P11) |
| `proxy.webservice.authentication-url` | value | planned (P11) |
| `proxy.webservice.groups-expression` | value | planned (P11) |
| `server.frame-options` | value | planned (P4) |
| `server.secure-cookies` | value | planned (P4) |
| `server.servlet.context-path` | value | planned (P4) |
| `server.undertow.max-http-post-size` | value | unsupported: Undertow specific |
| `server.use-forward-headers` | value | unsupported: removed in ShinyProxy 3.x |
| `spring.application.name` | value | supported |
| `spring.config.location` | value | supported |
| `spring.data.redis.database` | value | planned (P12) |
| `spring.data.redis.host` | value | planned (P12) |
| `spring.data.redis.password` | value | planned (P12) |
| `spring.data.redis.port` | value | planned (P12) |
| `spring.data.redis.sentinel.master` | value | planned (P12) |
| `spring.data.redis.sentinel.nodes` | list of values | planned (P12) |
| `spring.data.redis.sentinel.password` | value | planned (P12) |
| `spring.data.redis.username` | value | planned (P12) |
| `spring.mail.host` | value | planned (P7) |
| `spring.mail.password` | value | planned (P7) |
| `spring.mail.port` | value | planned (P7) |
| `spring.mail.properties` | map (free form keys) | planned (P7) |
| `spring.mail.username` | value | planned (P7) |
| `spring.profiles.active` | value | supported |
| `spring.servlet.multipart.enabled` | value | unsupported: bodies are never buffered |
| `spring.session.redis.flush-mode` | value | planned (P12) |
| `spring.session.redis.repository-type` | value | planned (P12) |
| `spring.session.store-type` | value | planned (P12) |
| `spring.session.timeout` | value | planned (P4) |
| `springdoc.api-docs.enabled` | value | planned (P7) |
| `springdoc.swagger-ui.enabled` | value | planned (P7) |

## Notes

* `proxy.ldap` accepts a single provider (`proxy.ldap.url`) or a list of providers (`proxy.ldap[0].url`).
* `proxy.ecs.enable-cloudwatch` is accepted as an alias of `proxy.ecs.enable-cloud-watch`.
* `proxy.container-backend: local` is an addition of this implementation: it starts apps as local processes and exists for testing only.
* Deviations from the Java implementation are tracked in [COMPATIBILITY.md](COMPATIBILITY.md).
