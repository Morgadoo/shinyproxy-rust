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
# Starts an OpenLDAP container with the fixtures the LDAP tests expect and keeps it running:
#
#   ./scripts/start-test-ldap.sh
#   SP_TEST_LDAP=1 cargo test -p shinyproxy --test ldap
#
# The directory contains uid=jack (member of scientists and admins) and uid=jeff (member of nothing),
# both with the password `password`, and cn=admin,dc=example,dc=com with the password `admin`.

set -euo pipefail

name="${SP_TEST_LDAP_CONTAINER:-test-ldap}"
port="${SP_TEST_LDAP_PORT:-3899}"

docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p "$port:389" \
    -e LDAP_ORGANISATION="Example" \
    -e LDAP_DOMAIN="example.com" \
    -e LDAP_ADMIN_PASSWORD="admin" \
    osixia/openldap:1.5.0 --copy-service >/dev/null

echo "waiting for the directory to accept connections"
for _ in $(seq 1 60); do
    if docker exec "$name" ldapsearch -x -H ldap://localhost -b dc=example,dc=com \
        -D "cn=admin,dc=example,dc=com" -w admin >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

seed="$(mktemp)"
trap 'rm -f "$seed"' EXIT
cat > "$seed" <<'LDIF'
dn: ou=people,dc=example,dc=com
objectClass: organizationalUnit
ou: people

dn: ou=groups,dc=example,dc=com
objectClass: organizationalUnit
ou: groups

dn: uid=jack,ou=people,dc=example,dc=com
objectClass: inetOrgPerson
uid: jack
cn: Jack
sn: Jackson
mail: jack@example.com
userPassword: password

dn: uid=jeff,ou=people,dc=example,dc=com
objectClass: inetOrgPerson
uid: jeff
cn: Jeff
sn: Jefferson
mail: jeff@example.com
userPassword: password

dn: cn=scientists,ou=groups,dc=example,dc=com
objectClass: groupOfNames
cn: scientists
member: uid=jack,ou=people,dc=example,dc=com

dn: cn=admins,ou=groups,dc=example,dc=com
objectClass: groupOfNames
cn: admins
member: uid=jack,ou=people,dc=example,dc=com
LDIF

docker cp "$seed" "$name:/tmp/seed.ldif" >/dev/null
docker exec "$name" ldapadd -x -H ldap://localhost -D "cn=admin,dc=example,dc=com" -w admin \
    -f /tmp/seed.ldif >/dev/null

echo "the directory is available on ldap://127.0.0.1:$port/dc=example,dc=com"
