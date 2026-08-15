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

//! App parameters (`ParametersService`).
//!
//! An app definition can ask the user for parameters before the app starts (`parameters.definitions`), and
//! `parameters.value-sets` says which combinations of values are allowed, optionally per group of users.
//! This module contains the three things the rest of the server needs:
//!
//! * [`validate_spec`] — the startup validation, with the same messages as the Java implementation, so
//!   that a broken configuration is refused instead of silently ignored.
//! * [`parse_and_validate_request`] — turns the values the user chose (human friendly names) into the
//!   values the backend gets, refusing combinations the user may not use.
//! * [`allowed_parameters_for_user`] — the data the app page needs to render the parameter form: the
//!   allowed values per parameter, the allowed combinations as index lists, and the default selection.
//!
//! Access control of value sets is passed in as a closure, because evaluating it needs the expression
//! context of the server (groups, users and SpEL expressions).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::spec::{AccessControl, ParameterDefinition, ParameterValueSet, ProxySpec};

/// The parameter values a user chose, as stored in the `SHINYPROXY_PARAMETERS` runtime value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterValues {
    /// Parameter id to the value the backend gets.
    pub backend_values: BTreeMap<String, String>,
    /// Name of the value set the values come from, when it has one.
    pub value_set_name: Option<String>,
}

impl ParameterValues {
    /// The value of a parameter.
    pub fn value(&self, parameter_id: &str) -> Option<&str> {
        self.backend_values.get(parameter_id).map(String::as_str)
    }
}

/// One parameter as shown to the user (`SHINYPROXY_PARAMETER_NAMES`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterName {
    /// Display name (or the id when there is none).
    pub display_name: String,
    /// Description of the parameter.
    pub description: Option<String>,
    /// The value the user chose (the human friendly name).
    pub value: Option<String>,
}

/// The parameters a user chose, as shown by the API.
///
/// Serialised as a plain list, exactly like the Java `ParameterNames` (`@JsonValue`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParameterNames(pub Vec<ParameterName>);

/// The parameters a user may choose from, used to render the form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedParametersForUser {
    /// Parameter id to the values the user may choose (human friendly names, in order of the index).
    pub values: BTreeMap<String, Vec<String>>,
    /// The allowed combinations, as lists of one-based value indexes (one entry per parameter).
    pub allowed_combinations: Vec<Vec<usize>>,
    /// The selection to show, as one-based value indexes; `0` means "no default".
    pub default_value: Vec<usize>,
}

/// The user chose parameters that are not allowed.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct InvalidParameters(pub String);

/// Parameter ids may only contain Latin letters, numbers, dash and underscore.
fn is_valid_parameter_id(id: &str) -> bool {
    id.chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

/// Validates the parameters of an app definition, with the messages of the Java implementation.
pub fn validate_spec(spec: &ProxySpec) -> Result<(), String> {
    let Some(parameters) = &spec.parameters else {
        return Ok(());
    };
    let error = |message: String| {
        Err(format!(
            "Configuration error: error in parameters of spec '{}', {message}",
            spec.id
        ))
    };

    if parameters.definitions.is_empty() {
        return error("no definitions found".to_string());
    }
    if parameters.value_sets.is_empty() {
        return error("no value sets found".to_string());
    }

    let mut ids = BTreeSet::new();
    for definition in &parameters.definitions {
        if definition.id.is_empty() {
            return error("error: id of parameter may not be null".to_string());
        }
        if !ids.insert(definition.id.clone()) {
            return error(format!("error: duplicate parameter id '{}'", definition.id));
        }
        if !is_valid_parameter_id(&definition.id) {
            return error(format!(
                "error: parameter id '{}' is invalid, id may only exists out of Latin letters, \
                 numbers, dash and underscore",
                definition.id
            ));
        }
        if definition
            .display_name
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return error(format!(
                "error: displayName may not be blank of parameter with id '{}'",
                definition.id
            ));
        }
        if definition
            .description
            .as_ref()
            .is_some_and(|description| description.trim().is_empty())
        {
            return error(format!(
                "error: description may not be blank of parameter with id '{}'",
                definition.id
            ));
        }
    }

    // either every parameter has a default value, or none has
    let defaults = parameters
        .definitions
        .iter()
        .map(|definition| definition.default_value.as_ref());
    let with_default = defaults.clone().filter(Option::is_some).count();
    if with_default != 0 && with_default != parameters.definitions.len() {
        return error(
            "error: not every parameter has a default value. Either define no defaults, or defaults \
             for all parameters."
                .to_string(),
        );
    }

    let parameter_ids = parameters.ids();
    for (index, value_set) in parameters.value_sets.iter().enumerate() {
        for parameter_id in &parameter_ids {
            if !value_set.contains_parameter(parameter_id) {
                return error(format!(
                    "error: value set {index} is missing values for parameter with id '{parameter_id}'"
                ));
            }
            let values = value_set.values_of(parameter_id);
            let unique: BTreeSet<&String> = values.iter().collect();
            if values.len() != unique.len() {
                return error(format!(
                    "error: value set {index} contains some duplicate values for parameter \
                     {parameter_id}"
                ));
            }
        }
        if value_set.values.len() != parameter_ids.len() {
            return error(format!(
                "error: value set {index} contains values for more parameters than there are defined"
            ));
        }
    }

    // every default value must exist in a value set
    if parameters
        .definitions
        .first()
        .and_then(|definition| definition.default_value.as_ref())
        .is_some()
    {
        for definition in &parameters.definitions {
            let default = definition.default_value.clone().unwrap_or_default();
            let exists = parameters
                .value_sets
                .iter()
                .any(|value_set| value_set.values_of(&definition.id).contains(&default));
            if !exists {
                return error(format!(
                    "error: default value for parameter with id '{}' is not defined in a value-set",
                    definition.id
                ));
            }
        }
    }

    validate_template(spec, parameters.template.as_deref())?;

    Ok(())
}

/// Thymeleaf constructs that a configuration provided template may not use.
const THYMELEAF_CONSTRUCTS: &[&str] = &[
    "th:each",
    "th:text",
    "th:utext",
    "th:if",
    "th:unless",
    "th:with",
    "th:attr",
    "th:href",
    "th:src",
    "th:block",
    "th:object",
    "th:field",
    "th:classappend",
    "th:remove",
    "th:inline",
];

/// Refuses a `parameters.template` that is written in Thymeleaf.
///
/// The Java implementation renders configuration provided templates with Thymeleaf; this implementation
/// renders them with MiniJinja, which would silently emit the `th:` attributes as plain HTML. Failing at
/// startup with the list of constructs found is friendlier than a broken form, and the conversion is
/// documented in `docs/COMPATIBILITY.md`.
fn validate_template(spec: &ProxySpec, template: Option<&str>) -> Result<(), String> {
    let Some(template) = template else {
        return Ok(());
    };

    let mut found: Vec<&str> = THYMELEAF_CONSTRUCTS
        .iter()
        .copied()
        .filter(|construct| template.contains(construct))
        .collect();
    // `${...}` and `*{...}` are Thymeleaf expressions; `#{...}` is also valid in a ShinyProxy template
    // (SpEL), so it is not reported
    if template.contains("${") {
        found.push("${...}");
    }
    if template.contains("*{") {
        found.push("*{...}");
    }
    if found.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Configuration error: error in parameters of spec '{}', error: the template uses Thymeleaf \
         constructs ({}) which this implementation does not support; write the template with the \
         MiniJinja syntax instead (see docs/COMPATIBILITY.md, section Templates)",
        spec.id,
        found.join(", ")
    ))
}

/// Turns the values a user chose into the values the backend gets.
///
/// `has_access` decides whether the user may use a value set (its `access-control`).
pub fn parse_and_validate_request(
    spec: &ProxySpec,
    provided: Option<&BTreeMap<String, String>>,
    has_access: &dyn Fn(Option<&AccessControl>) -> bool,
) -> Result<Option<(ParameterNames, ParameterValues)>, InvalidParameters> {
    let Some(parameters) = &spec.parameters else {
        return Ok(None);
    };
    let Some(provided) = provided else {
        return Err(InvalidParameters(
            "No parameters provided, but proxy spec expects parameters".to_string(),
        ));
    };

    let ids = parameters.ids();
    if provided.len() != ids.len() {
        return Err(InvalidParameters(
            "Invalid number of parameters provided".to_string(),
        ));
    }
    for parameter_id in &ids {
        if !provided.contains_key(parameter_id) {
            return Err(InvalidParameters(format!(
                "Missing value for parameter {parameter_id}"
            )));
        }
    }

    for value_set in &parameters.value_sets {
        if !has_access(value_set.access_control.as_ref()) {
            continue;
        }
        if let Some(result) = convert_if_allowed(&parameters.definitions, value_set, provided) {
            return Ok(Some(result));
        }
    }

    Err(InvalidParameters(
        "Provided parameter values are not allowed".to_string(),
    ))
}

/// Converts the chosen values when this value set allows them.
fn convert_if_allowed(
    definitions: &[ParameterDefinition],
    value_set: &ParameterValueSet,
    provided: &BTreeMap<String, String>,
) -> Option<(ParameterNames, ParameterValues)> {
    let mut backend_values = BTreeMap::new();
    for definition in definitions {
        let provided_value = provided.get(&definition.id)?;
        let backend_value = match definition.value_of_name(provided_value) {
            Some(value) => value.to_string(),
            None => {
                // the user provided a backend value; that is only allowed when the value has no name
                if definition.name_of_value(provided_value).is_some() {
                    return None;
                }
                provided_value.clone()
            }
        };
        if !value_set.values_of(&definition.id).contains(&backend_value) {
            return None;
        }
        backend_values.insert(definition.id.clone(), backend_value);
    }

    let names = ParameterNames(
        definitions
            .iter()
            .map(|definition| ParameterName {
                display_name: definition.display_name_or_id().to_string(),
                description: definition.description.clone(),
                value: provided.get(&definition.id).cloned(),
            })
            .collect(),
    );
    let values = ParameterValues {
        backend_values,
        value_set_name: value_set.name.clone(),
    };
    Some((names, values))
}

/// The values, combinations and default selection a user may choose from.
pub fn allowed_parameters_for_user(
    spec: &ProxySpec,
    previous: Option<&ParameterValues>,
    has_access: &dyn Fn(Option<&AccessControl>) -> bool,
) -> AllowedParametersForUser {
    let Some(parameters) = &spec.parameters else {
        return AllowedParametersForUser::default();
    };
    let ids = parameters.ids();

    // 1. the value sets this user may use
    let allowed_value_sets: Vec<&ParameterValueSet> = parameters
        .value_sets
        .iter()
        .filter(|value_set| has_access(value_set.access_control.as_ref()))
        .collect();

    // 2. a unique one-based index per value, per parameter (the front-end works with these indexes)
    let mut values_to_index: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for value_set in &allowed_value_sets {
        for definition in &parameters.definitions {
            let indexes = values_to_index.entry(definition.id.clone()).or_default();
            let names = values.entry(definition.id.clone()).or_default();
            for value in value_set.values_of(&definition.id) {
                let value = value.clone();
                if !indexes.contains_key(&value) {
                    indexes.insert(value.clone(), names.len() + 1);
                    names.push(
                        definition
                            .name_of_value(&value)
                            .unwrap_or(&value)
                            .to_string(),
                    );
                }
            }
        }
    }

    // 3. every allowed combination of indexes
    let mut allowed_combinations: Vec<Vec<usize>> = Vec::new();
    for value_set in &allowed_value_sets {
        for combination in combinations_of_value_set(&ids, value_set, &values_to_index) {
            if !allowed_combinations.contains(&combination) {
                allowed_combinations.push(combination);
            }
        }
    }

    // 4. the selection to show
    let default_value = default_selection(
        &parameters.definitions,
        &allowed_combinations,
        &values_to_index,
        previous,
    );

    AllowedParametersForUser {
        values,
        allowed_combinations,
        default_value,
    }
}

/// The combinations one value set allows (the cartesian product of its values).
fn combinations_of_value_set(
    parameter_ids: &[String],
    value_set: &ParameterValueSet,
    values_to_index: &HashMap<String, HashMap<String, usize>>,
) -> Vec<Vec<usize>> {
    let mut result: Vec<Vec<usize>> = vec![Vec::new()];
    for parameter_id in parameter_ids {
        let previous = std::mem::take(&mut result);
        for value in value_set.values_of(parameter_id) {
            let Some(index) = values_to_index
                .get(parameter_id)
                .and_then(|indexes| indexes.get(value))
            else {
                continue;
            };
            for combination in &previous {
                let mut extended = combination.clone();
                extended.push(*index);
                result.push(extended);
            }
        }
    }
    result
}

/// The selection to show: the values of the previous app, else the configured defaults, else nothing.
fn default_selection(
    definitions: &[ParameterDefinition],
    allowed_combinations: &[Vec<usize>],
    values_to_index: &HashMap<String, HashMap<String, usize>>,
    previous: Option<&ParameterValues>,
) -> Vec<usize> {
    let no_default = vec![0; definitions.len()];

    if let Some(previous) =
        previously_used(definitions, allowed_combinations, values_to_index, previous)
    {
        return previous;
    }

    if definitions
        .first()
        .and_then(|definition| definition.default_value.as_ref())
        .is_none()
    {
        return no_default;
    }

    let mut result = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let default = definition.default_value.clone().unwrap_or_default();
        match values_to_index
            .get(&definition.id)
            .and_then(|indexes| indexes.get(&default))
        {
            Some(index) => result.push(*index),
            // the default value cannot be used by this user
            None => return no_default,
        }
    }
    if allowed_combinations.contains(&result) {
        result
    } else {
        no_default
    }
}

/// The values of the app the user ran before, when they are still allowed.
fn previously_used(
    definitions: &[ParameterDefinition],
    allowed_combinations: &[Vec<usize>],
    values_to_index: &HashMap<String, HashMap<String, usize>>,
    previous: Option<&ParameterValues>,
) -> Option<Vec<usize>> {
    let previous = previous?;
    let mut result = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let value = previous.backend_values.get(&definition.id)?;
        let index = values_to_index
            .get(&definition.id)
            .and_then(|indexes| indexes.get(value))?;
        result.push(*index);
    }
    if allowed_combinations.contains(&result) {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a spec with the parameters written in YAML (as they appear in `application.yml`).
    fn build_spec(parameters_yaml: &str) -> ProxySpec {
        let mut spec = ProxySpec::new("app");
        let trimmed = parameters_yaml.trim();
        if !trimmed.is_empty() {
            spec.parameters =
                Some(serde_yaml_ng::from_str(trimmed).expect("parameters deserialize"));
        }
        spec
    }

    const PARAMETERS: &str = r##"
definitions:
  - id: environment
    display-name: Environment
    value-names:
      - value: base_r
        name: Base R
      - value: breeding_r
        name: Breeding R
  - id: memory
value-sets:
  - name: the-first-value-set
    values:
      environment: base_r
      memory: [ 2G, 4G ]
  - values:
      environment: breeding_r
      memory: 8G
    access-control:
      groups: breeding
"##;

    fn allow_all(_access: Option<&AccessControl>) -> bool {
        true
    }

    fn only_open(access: Option<&AccessControl>) -> bool {
        access.map(AccessControl::is_open).unwrap_or(true)
    }

    #[test]
    fn accepts_a_valid_configuration() {
        validate_spec(&build_spec(PARAMETERS)).expect("valid");
        // an app without parameters is valid as well
        validate_spec(&build_spec("")).expect("valid");
    }

    #[test]
    fn reports_configuration_errors_like_java() {
        let cases = [
            ("value-sets:\n  - values: {}\n", "no definitions found"),
            ("definitions:\n  - id: a\n", "no value sets found"),
            (
                "definitions:\n  - id: a\n  - id: a\nvalue-sets:\n  - values:\n      a: [ 1 ]\n",
                "duplicate parameter id 'a'",
            ),
            (
                "definitions:\n  - id: 'a b'\nvalue-sets:\n  - values:\n      'a b': [ 1 ]\n",
                "is invalid, id may only exists out of Latin letters",
            ),
            (
                "definitions:\n  - id: a\n    display-name: '  '\nvalue-sets:\n  - values:\n      a: [ 1 ]\n",
                "displayName may not be blank",
            ),
            (
                "definitions:\n  - id: a\n    description: '  '\nvalue-sets:\n  - values:\n      a: [ 1 ]\n",
                "description may not be blank",
            ),
            (
                "definitions:\n  - id: a\n    default-value: '1'\n  - id: b\nvalue-sets:\n  - values:\n      a: [ 1 ]\n      b: [ 2 ]\n",
                "not every parameter has a default value",
            ),
            (
                "definitions:\n  - id: a\n  - id: b\nvalue-sets:\n  - values:\n      a: [ 1 ]\n",
                "value set 0 is missing values for parameter with id 'b'",
            ),
            (
                "definitions:\n  - id: a\nvalue-sets:\n  - values:\n      a: [ 1, 1 ]\n",
                "contains some duplicate values for parameter a",
            ),
            (
                "definitions:\n  - id: a\nvalue-sets:\n  - values:\n      a: [ 1 ]\n      b: [ 2 ]\n",
                "contains values for more parameters than there are defined",
            ),
            (
                "definitions:\n  - id: a\n    default-value: '9'\nvalue-sets:\n  - values:\n      a: [ 1 ]\n",
                "default value for parameter with id 'a' is not defined in a value-set",
            ),
        ];

        for (yaml, expected) in cases {
            let error = validate_spec(&build_spec(yaml)).expect_err(expected);
            assert!(
                error.contains(expected),
                "expected '{expected}' in '{error}'"
            );
            assert!(
                error.starts_with("Configuration error: error in parameters of spec 'app'"),
                "{error}"
            );
        }
    }

    #[test]
    fn refuses_thymeleaf_templates() {
        let mut spec = build_spec(PARAMETERS);
        let mut parameters = spec.parameters.clone().expect("parameters");
        parameters.template = Some(
            "<div th:each=\"parameter : ${parameterDefinitions}\">\n  <span th:text=\"${parameter.id}\"></span>\n</div>"
                .to_string(),
        );
        spec.parameters = Some(parameters.clone());
        let error = validate_spec(&spec).expect_err("thymeleaf is refused");
        assert!(error.contains("th:each"), "{error}");
        assert!(error.contains("th:text"), "{error}");
        assert!(error.contains("${...}"), "{error}");
        assert!(error.contains("MiniJinja"), "{error}");

        // the MiniJinja version of the same template is accepted, including SpEL values
        parameters.template = Some(
            "{% for parameter in parameterDefinitions %}<span>{{ parameter.displayNameOrId }}</span>\
             {% endfor %}<span>#{userId}</span>"
                .to_string(),
        );
        spec.parameters = Some(parameters);
        validate_spec(&spec).expect("valid");
    }

    #[test]
    fn converts_chosen_values_into_backend_values() {
        let spec = build_spec(PARAMETERS);
        let provided = BTreeMap::from([
            ("environment".to_string(), "Base R".to_string()),
            ("memory".to_string(), "4G".to_string()),
        ]);
        let (names, values) = parse_and_validate_request(&spec, Some(&provided), &allow_all)
            .expect("valid")
            .expect("parameters");

        assert_eq!(
            values.backend_values.get("environment").map(String::as_str),
            Some("base_r")
        );
        assert_eq!(
            values.backend_values.get("memory").map(String::as_str),
            Some("4G")
        );
        assert_eq!(
            values.value_set_name.as_deref(),
            Some("the-first-value-set")
        );

        assert_eq!(names.0.len(), 2);
        assert_eq!(names.0[0].display_name, "Environment");
        assert_eq!(names.0[0].value.as_deref(), Some("Base R"));
        assert_eq!(
            names.0[1].display_name, "memory",
            "the id is used when there is no display name"
        );

        // the API serialises the names as a plain list
        let json = serde_json::to_value(&names).expect("json");
        assert!(json.is_array(), "{json}");
        assert_eq!(json[0]["displayName"], "Environment");
        assert_eq!(json[0]["value"], "Base R");
    }

    #[test]
    fn refuses_values_that_are_not_allowed() {
        let spec = build_spec(PARAMETERS);

        // no parameters at all
        let error = parse_and_validate_request(&spec, None, &allow_all).unwrap_err();
        assert_eq!(
            error.0,
            "No parameters provided, but proxy spec expects parameters"
        );

        // too few
        let provided = BTreeMap::from([("environment".to_string(), "Base R".to_string())]);
        let error = parse_and_validate_request(&spec, Some(&provided), &allow_all).unwrap_err();
        assert_eq!(error.0, "Invalid number of parameters provided");

        // the right number, but the wrong ids
        let provided = BTreeMap::from([
            ("environment".to_string(), "Base R".to_string()),
            ("other".to_string(), "4G".to_string()),
        ]);
        let error = parse_and_validate_request(&spec, Some(&provided), &allow_all).unwrap_err();
        assert_eq!(error.0, "Missing value for parameter memory");

        // a combination of two different value sets
        let provided = BTreeMap::from([
            ("environment".to_string(), "Base R".to_string()),
            ("memory".to_string(), "8G".to_string()),
        ]);
        let error = parse_and_validate_request(&spec, Some(&provided), &allow_all).unwrap_err();
        assert_eq!(error.0, "Provided parameter values are not allowed");

        // a value set the user may not use
        let provided = BTreeMap::from([
            ("environment".to_string(), "Breeding R".to_string()),
            ("memory".to_string(), "8G".to_string()),
        ]);
        parse_and_validate_request(&spec, Some(&provided), &allow_all)
            .expect("allowed for a member of the group");
        let error = parse_and_validate_request(&spec, Some(&provided), &only_open).unwrap_err();
        assert_eq!(error.0, "Provided parameter values are not allowed");

        // a backend value may not be used when the value has a name
        let provided = BTreeMap::from([
            ("environment".to_string(), "base_r".to_string()),
            ("memory".to_string(), "4G".to_string()),
        ]);
        let error = parse_and_validate_request(&spec, Some(&provided), &allow_all).unwrap_err();
        assert_eq!(error.0, "Provided parameter values are not allowed");
    }

    #[test]
    fn calculates_the_allowed_values_and_combinations() {
        let spec = build_spec(PARAMETERS);

        // a user who may use both value sets
        let allowed = allowed_parameters_for_user(&spec, None, &allow_all);
        assert_eq!(
            allowed.values.get("environment"),
            Some(&vec!["Base R".to_string(), "Breeding R".to_string()]),
            "the names are shown, in the order of the value sets"
        );
        assert_eq!(
            allowed.values.get("memory"),
            Some(&vec!["2G".to_string(), "4G".to_string(), "8G".to_string()])
        );
        // environment 1 with memory 1 and 2, environment 2 with memory 3
        assert_eq!(
            allowed.allowed_combinations,
            vec![vec![1, 1], vec![1, 2], vec![2, 3]]
        );
        assert_eq!(allowed.default_value, vec![0, 0], "no defaults configured");

        // a user who may only use the first value set
        let allowed = allowed_parameters_for_user(&spec, None, &only_open);
        assert_eq!(
            allowed.values.get("environment"),
            Some(&vec!["Base R".to_string()])
        );
        assert_eq!(
            allowed.values.get("memory"),
            Some(&vec!["2G".to_string(), "4G".to_string()])
        );
        assert_eq!(allowed.allowed_combinations, vec![vec![1, 1], vec![1, 2]]);

        // an app without parameters
        let allowed = allowed_parameters_for_user(&build_spec(""), None, &allow_all);
        assert_eq!(allowed, AllowedParametersForUser::default());
    }

    #[test]
    fn shows_the_configured_defaults() {
        let spec = build_spec(
            r##"
definitions:
  - id: environment
    default-value: breeding_r
    value-names:
      - value: base_r
        name: Base R
      - value: breeding_r
        name: Breeding R
  - id: memory
    default-value: 8G
value-sets:
  - values:
      environment: [ base_r, breeding_r ]
      memory: [ 2G, 8G ]
"##,
        );
        validate_spec(&spec).expect("valid");
        let allowed = allowed_parameters_for_user(&spec, None, &allow_all);
        assert_eq!(allowed.default_value, vec![2, 2]);

        // the values of the previous app win over the defaults
        let previous = ParameterValues {
            backend_values: BTreeMap::from([
                ("environment".to_string(), "base_r".to_string()),
                ("memory".to_string(), "2G".to_string()),
            ]),
            value_set_name: None,
        };
        let allowed = allowed_parameters_for_user(&spec, Some(&previous), &allow_all);
        assert_eq!(allowed.default_value, vec![1, 1]);

        // values the user may no longer use are ignored
        let previous = ParameterValues {
            backend_values: BTreeMap::from([
                ("environment".to_string(), "unknown".to_string()),
                ("memory".to_string(), "2G".to_string()),
            ]),
            value_set_name: None,
        };
        let allowed = allowed_parameters_for_user(&spec, Some(&previous), &allow_all);
        assert_eq!(allowed.default_value, vec![2, 2], "back to the defaults");
    }

    #[test]
    fn hides_defaults_the_user_may_not_use() {
        let spec = build_spec(
            r##"
definitions:
  - id: environment
    default-value: breeding_r
value-sets:
  - values:
      environment: base_r
  - values:
      environment: breeding_r
    access-control:
      groups: breeding
"##,
        );
        let allowed = allowed_parameters_for_user(&spec, None, &only_open);
        assert_eq!(allowed.default_value, vec![0]);
        let allowed = allowed_parameters_for_user(&spec, None, &allow_all);
        assert_eq!(allowed.default_value, vec![2]);
    }
}
