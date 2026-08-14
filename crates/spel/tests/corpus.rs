/*
 * ShinyProxy
 *
 * Copyright (C) 2016-2026 Open Analytics
 *
 * ===========================================================================
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the Apache License as published by
 * The Apache Software Foundation, either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * Apache License for more details.
 *
 * You should have received a copy of the Apache License
 * along with this program.  If not, see <http://www.apache.org/licenses/>
 */

//! Compatibility corpus for the expression engine.
//!
//! The expressions below are the ones ShinyProxy deployments actually use: they are taken from the
//! ShinyProxy documentation and configuration examples (container environment variables, access control
//! expressions, memory limits, volumes, Kubernetes pod patches, custom app details, titles) plus the
//! edge cases of the supported grammar. Each entry states the expected string result, so that a change
//! in the engine that would alter behaviour of a real configuration fails here.

use std::collections::BTreeMap;
use std::sync::Arc;

use spel::value::SpelObject;
use spel::{Context, SpelError, SpelErrorKind, Value};

/// A stand-in for the `proxy` object of the real context, with the methods configurations use.
#[derive(Debug)]
struct ProxyObject {
    id: String,
    user_id: String,
    spec_id: String,
    runtime_values: BTreeMap<String, String>,
}

impl SpelObject for ProxyObject {
    fn type_name(&self) -> &'static str {
        "proxy"
    }

    fn property(&self, name: &str) -> Option<Value> {
        match name {
            "id" => Some(Value::Str(self.id.clone())),
            "userId" => Some(Value::Str(self.user_id.clone())),
            "specId" => Some(Value::Str(self.spec_id.clone())),
            "status" => Some(Value::Str("Up".to_string())),
            _ => None,
        }
    }

    fn call(&self, name: &str, arguments: &[Value]) -> Option<Result<Value, SpelError>> {
        match (name, arguments.len()) {
            ("getRuntimeValue", 1) => {
                let key = arguments[0].to_display_string();
                Some(Ok(self
                    .runtime_values
                    .get(&key)
                    .cloned()
                    .map(Value::Str)
                    .unwrap_or(Value::Null)))
            }
            ("getRuntimeValueOrDefault", 2) => {
                let key = arguments[0].to_display_string();
                Some(Ok(self
                    .runtime_values
                    .get(&key)
                    .cloned()
                    .map(Value::Str)
                    .unwrap_or_else(|| arguments[1].clone())))
            }
            _ => None,
        }
    }

    fn to_display(&self) -> String {
        self.id.clone()
    }
}

fn context() -> Context {
    let proxy = Arc::new(ProxyObject {
        id: "5f39a7cf-c9ff-4a85-9313-d561ec79cca9".to_string(),
        user_id: "jack".to_string(),
        spec_id: "01_hello".to_string(),
        runtime_values: BTreeMap::from([
            ("SHINYPROXY_USERNAME".to_string(), "jack".to_string()),
            (
                "SHINYPROXY_USERGROUPS".to_string(),
                "SCIENTISTS,MATHEMATICIANS".to_string(),
            ),
            (
                "SHINYPROXY_PARAMETERS".to_string(),
                "{\"resources\":\"2-8\"}".to_string(),
            ),
        ]),
    });

    let oidc_attributes = Value::Map(BTreeMap::from([
        ("dept".to_string(), Value::Str("research".to_string())),
        (
            "email".to_string(),
            Value::Str("jack@example.com".to_string()),
        ),
        (
            "groups".to_string(),
            Value::List(vec![
                Value::Str("scientists".to_string()),
                Value::Str("admins".to_string()),
            ]),
        ),
        (
            "memberOf".to_string(),
            Value::Str("Research, Data Science".to_string()),
        ),
        ("quota".to_string(), Value::Int(4)),
    ]));

    let oidc_user = Value::Map(BTreeMap::from([
        ("attributes".to_string(), oidc_attributes.clone()),
        ("claims".to_string(), oidc_attributes),
        (
            "email".to_string(),
            Value::Str("jack@example.com".to_string()),
        ),
        ("name".to_string(), Value::Str("jack".to_string())),
    ]));

    let ldap_user = Value::Map(BTreeMap::from([(
        "attributes".to_string(),
        Value::Map(BTreeMap::from([(
            "memberOf".to_string(),
            Value::List(vec![Value::Str("cn=scientists,dc=example".to_string())]),
        )])),
    )]));

    let spec = Value::Map(BTreeMap::from([
        ("id".to_string(), Value::Str("01_hello".to_string())),
        (
            "displayName".to_string(),
            Value::Str("Hello Application".to_string()),
        ),
    ]));

    Context::new()
        .with_object("proxy", proxy)
        .with_root("proxySpec", spec)
        .with_root("userId", "jack")
        .with_root(
            "groups",
            vec!["SCIENTISTS".to_string(), "MATHEMATICIANS".to_string()],
        )
        .with_root("oidcUser", oidc_user)
        .with_root("ldapUser", ldap_user)
        .with_root("serverName", "shinyproxy.example.com")
        .with_environment(BTreeMap::from([
            ("HOME".to_string(), "/home/shinyproxy".to_string()),
            ("SP_DATA_DIR".to_string(), "/data".to_string()),
        ]))
}

/// Expressions and their expected string result.
const CORPUS: &[(&str, &str)] = &[
    // --- literals and arithmetic ---
    ("#{1}", "1"),
    ("#{1 + 2}", "3"),
    ("#{2 * 3 + 1}", "7"),
    ("#{(2 + 3) * 4}", "20"),
    ("#{10 / 4}", "2"),
    ("#{10.0 / 4}", "2.5"),
    ("#{10 % 3}", "1"),
    ("#{-5 + 1}", "-4"),
    ("#{1.5 + 1.5}", "3.0"),
    ("#{'a' + 'b' + 'c'}", "abc"),
    ("#{'value: ' + 42}", "value: 42"),
    ("#{true}", "true"),
    ("#{!true}", "false"),
    ("#{null}", "null"),
    ("#{'it''s'}", "it's"),
    ("#{\"double\"}", "double"),
    // --- plain text and mixed templates ---
    ("plain", "plain"),
    ("", ""),
    ("/home/#{userId}", "/home/jack"),
    ("#{userId}-#{proxySpec.id}", "jack-01_hello"),
    ("app-#{userId}.example.com", "app-jack.example.com"),
    ("#{userId}", "jack"),
    // --- context values used by ShinyProxy configurations ---
    ("#{proxy.id}", "5f39a7cf-c9ff-4a85-9313-d561ec79cca9"),
    ("#{proxy.userId}", "jack"),
    ("#{proxy.specId}", "01_hello"),
    ("#{proxy.status}", "Up"),
    ("#{proxySpec.id}", "01_hello"),
    ("#{proxySpec.displayName}", "Hello Application"),
    ("#{serverName}", "shinyproxy.example.com"),
    ("#{groups[0]}", "SCIENTISTS"),
    ("#{groups.size()}", "2"),
    ("#{groups.isEmpty()}", "false"),
    ("#{groups.contains('SCIENTISTS')}", "true"),
    ("#{groups.contains('unknown')}", "false"),
    // --- runtime values ---
    ("#{proxy.getRuntimeValue('SHINYPROXY_USERNAME')}", "jack"),
    (
        "#{proxy.getRuntimeValue('SHINYPROXY_USERGROUPS')}",
        "SCIENTISTS,MATHEMATICIANS",
    ),
    ("#{proxy.getRuntimeValue('MISSING')}", "null"),
    (
        "#{proxy.getRuntimeValueOrDefault('MISSING', 'fallback')}",
        "fallback",
    ),
    (
        "#{proxy.getRuntimeValue('SHINYPROXY_PARAMETERS')}",
        "{\"resources\":\"2-8\"}",
    ),
    // --- OIDC / LDAP / SAML attributes ---
    ("#{oidcUser.attributes['dept']}", "research"),
    ("#{oidcUser.attributes['quota']}", "4"),
    ("#{oidcUser.email}", "jack@example.com"),
    ("#{oidcUser.attributes['groups'][1]}", "admins"),
    (
        "#{oidcUser.attributes['groups'].contains('admins')}",
        "true",
    ),
    ("#{oidcUser.claims['dept']}", "research"),
    ("#{oidcUser.attributes['missing']}", "null"),
    ("#{oidcUser?.attributes?.dept}", "research"),
    (
        "#{ldapUser.attributes['memberOf'][0]}",
        "cn=scientists,dc=example",
    ),
    // --- helper functions of the expression context ---
    ("#{toList('a, b, c').size()}", "3"),
    ("#{toList('a, b, c')[1]}", "b"),
    (
        "#{toList(oidcUser.attributes['memberOf'])[1]}",
        "Data Science",
    ),
    ("#{toList('a;b', ';')[0]}", "a"),
    ("#{toLowerCaseList('A, B')[0]}", "a"),
    ("#{toLowerCaseList(groups).contains('scientists')}", "true"),
    ("#{toLowerCaseList('A;B', ';')[1]}", "b"),
    ("#{isOneOf(userId, 'jack', 'jill')}", "true"),
    ("#{isOneOf(userId, 'jill')}", "false"),
    (
        "#{isOneOf(oidcUser.attributes['dept'], 'research', 'dev')}",
        "true",
    ),
    ("#{isOneOfIgnoreCase(userId, 'JACK')}", "true"),
    ("#{isOneOf(oidcUser.attributes['missing'], 'a')}", "false"),
    // --- string methods ---
    ("#{userId.toUpperCase()}", "JACK"),
    ("#{userId.toLowerCase()}", "jack"),
    ("#{userId.length()}", "4"),
    ("#{userId.substring(0, 2)}", "ja"),
    ("#{userId.substring(2)}", "ck"),
    ("#{userId.startsWith('ja')}", "true"),
    ("#{userId.endsWith('ck')}", "true"),
    ("#{userId.contains('ac')}", "true"),
    ("#{userId.replace('ja', 'JA')}", "JAck"),
    ("#{userId.equals('jack')}", "true"),
    ("#{userId.equalsIgnoreCase('JACK')}", "true"),
    ("#{'  padded '.trim()}", "padded"),
    ("#{'a,b,c'.split(',').size()}", "3"),
    ("#{'a,b,c'.split(',')[2]}", "c"),
    (
        "#{oidcUser.attributes['email'].split('@')[1]}",
        "example.com",
    ),
    ("#{userId.indexOf('c')}", "2"),
    ("#{userId.isEmpty()}", "false"),
    ("#{userId.matches('ja.*')}", "true"),
    ("#{userId.matches('.*x.*')}", "false"),
    ("#{userId.replaceAll('[aeiou]', '*')}", "j*ck"),
    // --- comparisons and logic used in access expressions ---
    ("#{userId == 'jack'}", "true"),
    ("#{userId != 'jack'}", "false"),
    ("#{userId eq 'jack'}", "true"),
    ("#{userId ne 'jill'}", "true"),
    ("#{1 < 2}", "true"),
    ("#{2 <= 2}", "true"),
    ("#{3 > 4}", "false"),
    ("#{4 >= 4}", "true"),
    ("#{1 lt 2}", "true"),
    ("#{2 gt 1}", "true"),
    (
        "#{groups.contains('SCIENTISTS') and userId == 'jack'}",
        "true",
    ),
    ("#{groups.contains('nope') or userId == 'jack'}", "true"),
    ("#{groups.contains('nope') || false}", "false"),
    ("#{!(userId == 'jill')}", "true"),
    ("#{not (userId == 'jill')}", "true"),
    (
        "#{groups.contains('SCIENTISTS') and oidcUser.attributes['dept'] == 'research'}",
        "true",
    ),
    ("#{oidcUser.attributes['quota'] > 2}", "true"),
    ("#{userId matches '^j.*k$'}", "true"),
    // --- ternary, elvis, defaults ---
    ("#{userId == 'jack' ? '4g' : '2g'}", "4g"),
    ("#{userId == 'jill' ? '4g' : '2g'}", "2g"),
    ("#{oidcUser.attributes['missing'] ?: 'default'}", "default"),
    ("#{oidcUser.attributes['dept'] ?: 'default'}", "research"),
    (
        "#{groups.contains('SCIENTISTS') ? 'unlimited' : '1'}",
        "unlimited",
    ),
    // --- collections, projection, selection ---
    ("#{{1, 2, 3}.size()}", "3"),
    ("#{{}.isEmpty()}", "true"),
    ("#{{'a': 1}['a']}", "1"),
    ("#{{'a': 1}.containsKey('a')}", "true"),
    ("#{groups.![#this.toLowerCase()][0]}", "scientists"),
    ("#{groups.?[#this == 'SCIENTISTS'].size()}", "1"),
    (
        "#{oidcUser.attributes['groups'].?[#this matches 'a.*'][0]}",
        "admins",
    ),
    ("#{groups.![#this.length()][1]}", "14"),
    // --- allow-listed java types ---
    ("#{T(java.lang.System).getenv('HOME')}", "/home/shinyproxy"),
    (
        "#{T(java.lang.System).getenv('SP_DATA_DIR')}/#{userId}",
        "/data/jack",
    ),
    ("#{T(java.lang.System).getenv('NOT_SET')}", "null"),
    ("#{T(java.lang.String).valueOf(42)}", "42"),
    (
        "#{T(java.lang.String).join('-', groups)}",
        "SCIENTISTS-MATHEMATICIANS",
    ),
    ("#{T(java.lang.Math).max(1, 2)}", "2"),
    ("#{T(java.lang.Math).abs(-3)}", "3"),
    ("#{T(java.lang.Integer).parseInt('7') + 1}", "8"),
    ("#{T(java.lang.Boolean).parseBoolean('true')}", "true"),
    // --- realistic full values from documentation examples ---
    ("/home/#{userId}/workspace", "/home/jack/workspace"),
    (
        "#{userId == 'jack' ? '/data/private' : '/data/public'}:/data",
        "/data/private:/data",
    ),
    (
        "shinyproxy-#{proxy.specId}-#{userId}",
        "shinyproxy-01_hello-jack",
    ),
    (
        "#{groups.contains('SCIENTISTS') ? 'nvidia' : 'runc'}",
        "nvidia",
    ),
    ("Welcome #{userId}!", "Welcome jack!"),
    (
        "#{oidcUser.attributes['dept'].toLowerCase()}-namespace",
        "research-namespace",
    ),
];

#[test]
fn corpus_evaluates_as_expected() {
    let context = context();
    assert!(
        CORPUS.len() >= 120,
        "corpus must stay comprehensive, has {}",
        CORPUS.len()
    );
    for (expression, expected) in CORPUS {
        let actual = spel::evaluate_template(expression, &context)
            .unwrap_or_else(|error| panic!("{expression} failed: {error}"));
        assert_eq!(&actual, expected, "expression: {expression}");
    }
}

#[test]
fn typed_conversions_of_corpus_expressions() {
    let context = context();

    // integers (used for max-instances, max-lifetime, heartbeat-timeout, ports)
    assert_eq!(spel::evaluate_to_integer("#{1 + 1}", &context).unwrap(), 2);
    assert_eq!(spel::evaluate_to_integer("120", &context).unwrap(), 120);
    assert_eq!(
        spel::evaluate_to_integer("#{groups.contains('SCIENTISTS') ? 10 : 1}", &context).unwrap(),
        10
    );
    assert_eq!(
        spel::evaluate_to_integer("#{oidcUser.attributes['quota']}", &context).unwrap(),
        4
    );

    // booleans (used for access expressions and feature toggles)
    assert!(spel::evaluate_to_boolean("#{userId == 'jack'}", &context).unwrap());
    assert!(!spel::evaluate_to_boolean("#{userId == 'jill'}", &context).unwrap());
    assert!(spel::evaluate_to_boolean("true", &context).unwrap());

    // lists (used for cmd, volumes, dns, ...)
    assert_eq!(
        spel::evaluate_to_list("#{groups}", &context).unwrap(),
        vec!["SCIENTISTS".to_string(), "MATHEMATICIANS".to_string()]
    );
    assert_eq!(
        spel::evaluate_to_list("#{toLowerCaseList(groups)}", &context).unwrap(),
        vec!["scientists".to_string(), "mathematicians".to_string()]
    );
}

/// Expressions that must fail, with the message fragment that makes the failure actionable.
const FAILURES: &[(&str, SpelErrorKind, &str)] = &[
    (
        "#{unknownValue}",
        SpelErrorKind::Unknown,
        "available values",
    ),
    (
        "#{proxy.unknownProperty}",
        SpelErrorKind::Unknown,
        "has no property",
    ),
    (
        "#{proxy.unknownMethod()}",
        SpelErrorKind::Unknown,
        "has no method",
    ),
    (
        "#{userId.unknownMethod()}",
        SpelErrorKind::Unknown,
        "a string has no method",
    ),
    ("#{nope()}", SpelErrorKind::Unknown, "unknown function"),
    ("#{1 +}", SpelErrorKind::Syntax, "expected a value"),
    ("#{'unterminated}", SpelErrorKind::Syntax, "unterminated"),
    (
        "#{userId ~ 'a'}",
        SpelErrorKind::Syntax,
        "unexpected character",
    ),
    (
        "#{new java.lang.String('a')}",
        SpelErrorKind::Unsupported,
        "constructor calls",
    ),
    (
        "#{T(java.io.File).createTempFile('a', 'b')}",
        SpelErrorKind::Unsupported,
        "not supported",
    ),
    ("#{1 / 0}", SpelErrorKind::Evaluation, "division by zero"),
    (
        "#{userId.length() + userId.nope}",
        SpelErrorKind::Unknown,
        "has no property",
    ),
];

#[test]
fn unsupported_expressions_fail_with_actionable_messages() {
    let context = context();
    for (expression, kind, fragment) in FAILURES {
        let error = spel::evaluate_template(expression, &context)
            .expect_err(&format!("{expression} must fail"));
        assert_eq!(&error.kind, kind, "expression: {expression} ({error})");
        assert!(
            error.message.contains(fragment) || error.to_string().contains(fragment),
            "expression {expression}: expected '{fragment}' in '{error}'"
        );
    }
}

/// The whole corpus is cross-validated against the real Spring Expression Language by
/// `tools/spel-crossvalidate/run.sh`; these are the only two constructs where this implementation is
/// deliberately *more* permissive than Spring, so configurations that work with Java keep working.
#[test]
fn documented_supersets_of_java_spel() {
    let context = context();

    // Spring only supports `map['key']` (the `MapAccessor` is not registered by default), this
    // implementation also supports `map.key`.
    assert_eq!(
        spel::evaluate_template("#{oidcUser?.attributes?.dept}", &context).unwrap(),
        "research"
    );

    // `String.split` returns an array in Java, which has `length` but no `size()`; here both work.
    assert_eq!(
        spel::evaluate_template("#{'a,b,c'.split(',').size()}", &context).unwrap(),
        "3"
    );
    assert_eq!(
        spel::evaluate_template("#{'a,b,c'.split(',').length}", &context).unwrap(),
        "3"
    );
}

#[test]
fn null_values_stringify_like_java() {
    let context = context();
    // Java renders a null result of a template as the string "null"
    assert_eq!(
        spel::evaluate_template("#{oidcUser.attributes['missing']}", &context).unwrap(),
        "null"
    );
    // ... and an empty string when the expression is the whole value and converted explicitly
    assert_eq!(
        spel::evaluate_template_to_value("#{oidcUser.attributes['missing']}", &context)
            .unwrap()
            .as_string("")
            .unwrap(),
        ""
    );
}
