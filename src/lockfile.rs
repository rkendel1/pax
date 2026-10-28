//! Static, format-aware lockfile observation.
//!
//! A lockfile that exists is not evidence that its recorded dependency set was
//! established. This module parses recognized formats and returns an explicit
//! `unknown` observation when a format cannot be interpreted. It never runs a
//! package manager and never mutates the project.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Cargo,
    Uv,
    Poetry,
    Pdm,
    Pipfile,
}

impl Format {
    pub(crate) fn detect(path: &str) -> Option<Self> {
        let name = Path::new(path).file_name()?.to_str()?;
        Some(match name {
            "package-lock.json" => Self::Npm,
            "pnpm-lock.yaml" => Self::Pnpm,
            "yarn.lock" => Self::Yarn,
            "bun.lock" | "bun.lockb" => Self::Bun,
            "Cargo.lock" => Self::Cargo,
            "uv.lock" => Self::Uv,
            "poetry.lock" => Self::Poetry,
            "pdm.lock" => Self::Pdm,
            "Pipfile.lock" => Self::Pipfile,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
            Self::Cargo => "cargo",
            Self::Uv => "uv",
            Self::Poetry => "poetry",
            Self::Pdm => "pdm",
            Self::Pipfile => "pipenv",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    pub(crate) format: Option<Format>,
    /// Canonical fact state: `present` when the recorded dependency set was
    /// established, `unknown` when the format could not be interpreted, and
    /// `failed` when the file could not be read.
    pub(crate) status: &'static str,
    pub(crate) packages: BTreeSet<String>,
    pub(crate) detail: Option<String>,
}

impl Observation {
    pub(crate) fn established(&self) -> bool {
        self.status == "present"
    }

    fn known(format: Format, packages: BTreeSet<String>) -> Self {
        Self {
            format: Some(format),
            status: "present",
            packages,
            detail: None,
        }
    }

    fn unknown(format: Option<Format>, detail: impl Into<String>) -> Self {
        Self {
            format,
            status: "unknown",
            packages: BTreeSet::new(),
            detail: Some(detail.into()),
        }
    }

    fn failed(format: Format, detail: impl Into<String>) -> Self {
        Self {
            format: Some(format),
            status: "failed",
            packages: BTreeSet::new(),
            detail: Some(detail.into()),
        }
    }
}

/// Reads and interprets one lockfile. The caller supplies the project-relative
/// display path so that parsing and evidence use the same identity.
pub(crate) fn observe(path: &Path, display: &str) -> Observation {
    let Some(format) = Format::detect(display) else {
        return Observation::unknown(None, "unrecognized lockfile format");
    };
    if format == Format::Bun
        && Path::new(display)
            .extension()
            .and_then(|value| value.to_str())
            == Some("lockb")
    {
        return Observation::unknown(
            Some(format),
            "binary bun lockfile is not interpreted; use bun.lock",
        );
    }
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            return Observation::failed(format, format!("read failed: {:?}", error.kind()));
        }
    };
    match parse(format, &contents) {
        Ok(packages) => Observation::known(format, packages),
        Err(detail) => Observation::unknown(Some(format), detail),
    }
}

fn parse(format: Format, contents: &str) -> Result<BTreeSet<String>, String> {
    match format {
        Format::Npm => parse_npm(contents),
        Format::Pnpm => parse_pnpm(contents),
        Format::Yarn => parse_yarn(contents),
        Format::Bun => parse_bun(contents),
        Format::Cargo | Format::Uv | Format::Poetry | Format::Pdm => parse_toml(contents),
        Format::Pipfile => parse_pipfile(contents),
    }
}

fn parse_npm(contents: &str) -> Result<BTreeSet<String>, String> {
    let value = serde_json::from_str::<serde_json::Value>(contents)
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    let has_structure = object.contains_key("lockfileVersion")
        || object
            .get("packages")
            .is_some_and(|value| value.is_object())
        || object
            .get("dependencies")
            .is_some_and(|value| value.is_object());
    if !has_structure {
        return Err("missing lockfileVersion, packages, or dependencies".to_string());
    }
    let mut packages = BTreeSet::new();
    if let Some(entries) = object.get("packages").and_then(|value| value.as_object()) {
        for key in entries.keys() {
            if let Some(name) = npm_package_name(key) {
                packages.insert(name);
            }
        }
    }
    if let Some(dependencies) = object
        .get("dependencies")
        .and_then(|value| value.as_object())
    {
        collect_npm_v1(dependencies, &mut packages);
    }
    Ok(packages)
}

fn npm_package_name(key: &str) -> Option<String> {
    let rest = key.strip_prefix("node_modules/")?;
    let rest = rest.rsplit("node_modules/").next().unwrap_or(rest);
    if rest.is_empty() || rest.starts_with('.') {
        return None;
    }
    if rest.starts_with('@') {
        let mut parts = rest.splitn(3, '/');
        let scope = parts.next()?;
        let name = parts.next()?;
        Some(format!("{scope}/{name}"))
    } else {
        rest.split('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }
}

fn collect_npm_v1(
    dependencies: &serde_json::Map<String, serde_json::Value>,
    packages: &mut BTreeSet<String>,
) {
    for (name, value) in dependencies {
        packages.insert(name.clone());
        if let Some(nested) = value
            .get("dependencies")
            .and_then(|value| value.as_object())
        {
            collect_npm_v1(nested, packages);
        }
    }
}

fn parse_pnpm(contents: &str) -> Result<BTreeSet<String>, String> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(contents)
        .map_err(|error| format!("invalid YAML: {error}"))?;
    let mapping = value
        .as_mapping()
        .ok_or_else(|| "expected a YAML mapping".to_string())?;
    let has_structure = yaml_get(mapping, "lockfileVersion").is_some()
        || yaml_get(mapping, "importers").is_some()
        || yaml_get(mapping, "packages").is_some()
        || yaml_get(mapping, "dependencies").is_some()
        || yaml_get(mapping, "devDependencies").is_some();
    if !has_structure {
        return Err("missing lockfileVersion, importers, or packages".to_string());
    }
    let mut packages = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        add_yaml_keys(yaml_get(mapping, section), &mut packages);
    }
    if let Some(importers) = yaml_get(mapping, "importers").and_then(serde_yaml::Value::as_mapping)
    {
        for importer in importers.values().filter_map(serde_yaml::Value::as_mapping) {
            for section in [
                "dependencies",
                "devDependencies",
                "optionalDependencies",
                "peerDependencies",
            ] {
                add_yaml_keys(yaml_get(importer, section), &mut packages);
            }
        }
    }
    if let Some(entries) = yaml_get(mapping, "packages").and_then(serde_yaml::Value::as_mapping) {
        for key in entries.keys().filter_map(serde_yaml::Value::as_str) {
            if let Some(name) = pnpm_package_name(key) {
                packages.insert(name);
            }
        }
    }
    Ok(packages)
}

fn yaml_get<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    mapping.get(serde_yaml::Value::String(key.to_string()))
}

fn add_yaml_keys(value: Option<&serde_yaml::Value>, packages: &mut BTreeSet<String>) {
    if let Some(mapping) = value.and_then(serde_yaml::Value::as_mapping) {
        for key in mapping.keys().filter_map(serde_yaml::Value::as_str) {
            if !key.is_empty() {
                packages.insert(key.to_string());
            }
        }
    }
}

fn pnpm_package_name(key: &str) -> Option<String> {
    let key = key.trim_start_matches('/');
    let key = key.split('(').next().unwrap_or(key);
    if key.is_empty() {
        return None;
    }
    if key.starts_with('@') {
        let (scope, rest) = key.split_once('/')?;
        let name = rest.split(['@', '/']).next()?;
        (!name.is_empty()).then(|| format!("{scope}/{name}"))
    } else {
        key.split(['@', '/'])
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }
}

fn parse_yarn(contents: &str) -> Result<BTreeSet<String>, String> {
    let mut packages = BTreeSet::new();
    let mut header = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# yarn lockfile") || trimmed == "__metadata:" {
            header = true;
        }
        if line.starts_with(' ')
            || line.starts_with('\t')
            || trimmed.is_empty()
            || trimmed.starts_with('#')
        {
            continue;
        }
        let Some(entry) = line.strip_suffix(':') else {
            continue;
        };
        for selector in entry.split(',') {
            let selector = selector.trim().trim_matches('"');
            if selector.is_empty() || selector == "__metadata" {
                continue;
            }
            if let Some(name) = yarn_package_name(selector) {
                packages.insert(name);
            }
        }
    }
    if packages.is_empty() && !header {
        return Err("no package entries and no yarn header".to_string());
    }
    Ok(packages)
}

fn yarn_package_name(selector: &str) -> Option<String> {
    let selector = selector.split_whitespace().next()?;
    if selector.starts_with('@') {
        let (scope, rest) = selector.split_once('/')?;
        let name = rest.split('@').next()?;
        (!name.is_empty()).then(|| format!("{scope}/{name}"))
    } else {
        selector
            .split('@')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }
}

fn parse_bun(contents: &str) -> Result<BTreeSet<String>, String> {
    let json = strip_jsonc(contents);
    let value = serde_json::from_str::<serde_json::Value>(&json)
        .map_err(|error| format!("invalid JSONC: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    let has_structure = object.contains_key("lockfileVersion")
        || object
            .get("packages")
            .is_some_and(|value| value.is_object())
        || object
            .get("workspaces")
            .is_some_and(|value| value.is_object());
    if !has_structure {
        return Err("missing lockfileVersion, packages, or workspaces".to_string());
    }
    let mut packages = BTreeSet::new();
    if let Some(entries) = object.get("packages").and_then(|value| value.as_object()) {
        for key in entries.keys() {
            if !key.is_empty() {
                packages.insert(key.clone());
            }
        }
    }
    if let Some(workspaces) = object.get("workspaces").and_then(|value| value.as_object()) {
        for workspace in workspaces.values().filter_map(|value| value.as_object()) {
            for section in [
                "dependencies",
                "devDependencies",
                "optionalDependencies",
                "peerDependencies",
            ] {
                add_json_keys(workspace.get(section), &mut packages);
            }
        }
    }
    if let Some(dependencies) = object
        .get("dependencies")
        .and_then(|value| value.as_object())
    {
        add_json_keys(
            Some(&serde_json::Value::Object(dependencies.clone())),
            &mut packages,
        );
    }
    Ok(packages)
}

fn add_json_keys(value: Option<&serde_json::Value>, packages: &mut BTreeSet<String>) {
    if let Some(mapping) = value.and_then(|value| value.as_object()) {
        for key in mapping.keys() {
            if !key.is_empty() {
                packages.insert(key.clone());
            }
        }
    }
}

/// Removes JSONC comments and trailing commas without touching string contents.
fn strip_jsonc(input: &str) -> String {
    let mut without_comments = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if in_string {
            without_comments.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                without_comments.push(character);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                while let Some(&next) = chars.peek() {
                    if next == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => without_comments.push(character),
        }
    }
    remove_trailing_commas(&without_comments)
}

fn remove_trailing_commas(input: &str) -> String {
    let characters = input.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in characters.iter().enumerate() {
        if in_string {
            output.push(*character);
            if escaped {
                escaped = false;
            } else if *character == '\\' {
                escaped = true;
            } else if *character == '"' {
                in_string = false;
            }
            continue;
        }
        if *character == '"' {
            in_string = true;
            output.push(*character);
            continue;
        }
        if *character == ',' {
            let mut next = index + 1;
            while next < characters.len() && characters[next].is_whitespace() {
                next += 1;
            }
            if next < characters.len() && matches!(characters[next], '}' | ']') {
                continue;
            }
        }
        output.push(*character);
    }
    output
}

fn parse_toml(contents: &str) -> Result<BTreeSet<String>, String> {
    let value = toml::from_str::<toml::Value>(contents)
        .map_err(|error| format!("invalid TOML: {error}"))?;
    let table = value
        .as_table()
        .ok_or_else(|| "expected a TOML table".to_string())?;
    let has_structure = table.contains_key("package")
        || table.contains_key("version")
        || table.contains_key("metadata");
    if !has_structure {
        return Err("missing package entries or lock metadata".to_string());
    }
    let mut packages = BTreeSet::new();
    if let Some(entries) = table.get("package").and_then(toml::Value::as_array) {
        for entry in entries {
            if let Some(name) = entry.get("name").and_then(toml::Value::as_str)
                && !name.is_empty()
            {
                packages.insert(name.to_string());
            }
        }
    }
    Ok(packages)
}

fn parse_pipfile(contents: &str) -> Result<BTreeSet<String>, String> {
    let value = serde_json::from_str::<serde_json::Value>(contents)
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    let has_structure = object.contains_key("_meta")
        || object.contains_key("default")
        || object.contains_key("develop");
    if !has_structure {
        return Err("missing _meta, default, or develop sections".to_string());
    }
    let mut packages = BTreeSet::new();
    for section in ["default", "develop"] {
        add_json_keys(object.get(section), &mut packages);
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_v2_and_v3_packages_are_interpreted() {
        let packages = parse(
            Format::Npm,
            r#"{"lockfileVersion":3,"packages":{"":{"name":"web"},"node_modules/left-pad":{"version":"1.3.0"},"node_modules/@scope/pkg":{"version":"2.0.0"}}}"#,
        )
        .unwrap();
        assert_eq!(
            packages,
            BTreeSet::from(["@scope/pkg".to_string(), "left-pad".to_string()])
        );
    }

    #[test]
    fn npm_v1_nested_dependencies_are_interpreted() {
        let packages = parse(
            Format::Npm,
            r#"{"dependencies":{"left-pad":{"version":"1.3.0","dependencies":{"nested":"1.0.0"}}}}"#,
        )
        .unwrap();
        assert_eq!(
            packages,
            BTreeSet::from(["left-pad".to_string(), "nested".to_string()])
        );
    }

    #[test]
    fn structurally_empty_lockfiles_are_unknown() {
        for (format, contents) in [
            (Format::Npm, "{}"),
            (Format::Pnpm, ""),
            (Format::Yarn, ""),
            (Format::Bun, "{}"),
            (Format::Cargo, ""),
            (Format::Uv, ""),
            (Format::Poetry, ""),
            (Format::Pdm, ""),
            (Format::Pipfile, "{}"),
        ] {
            assert!(parse(format, contents).is_err(), "{format:?}");
        }
    }

    #[test]
    fn pnpm_importers_and_package_keys_are_interpreted() {
        let packages = parse(
            Format::Pnpm,
            "lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      left-pad:\n        specifier: ^1.0.0\n        version: 1.3.0\npackages:\n  left-pad@1.3.0:\n    resolution: {integrity: sha512-x}\n  '@scope/pkg@2.0.0':\n    resolution: {integrity: sha512-y}\n",
        )
        .unwrap();
        assert_eq!(
            packages,
            BTreeSet::from([
                "@scope/pkg".to_string(),
                "left-pad".to_string(),
                "left-pad".to_string()
            ])
        );
    }

    #[test]
    fn yarn_v1_and_berry_entries_are_interpreted() {
        let v1 = parse(
            Format::Yarn,
            "# yarn lockfile v1\n\nleft-pad@^1.0.0:\n  version \"1.3.0\"\n\"@scope/pkg@^2.0.0\":\n  version \"2.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            v1,
            BTreeSet::from(["@scope/pkg".to_string(), "left-pad".to_string()])
        );
        let berry = parse(
            Format::Yarn,
            "__metadata:\n  version: 8\n\n\"left-pad@npm:^1.0.0\":\n  version: 1.3.0\n",
        )
        .unwrap();
        assert_eq!(berry, BTreeSet::from(["left-pad".to_string()]));
    }

    #[test]
    fn bun_jsonc_is_interpreted() {
        let packages = parse(
            Format::Bun,
            "{\n  // comment\n  \"lockfileVersion\": 1,\n  \"workspaces\": {\"\": {\"dependencies\": {\"react\": \"^19\"}}},\n  \"packages\": {\"left-pad\": [\"left-pad@1.3.0\", \"\", {}, \"sha512\"],},\n}\n",
        )
        .unwrap();
        assert_eq!(
            packages,
            BTreeSet::from(["left-pad".to_string(), "react".to_string()])
        );
    }

    #[test]
    fn toml_lockfiles_are_interpreted() {
        let packages = parse(
            Format::Cargo,
            "version = 3\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert_eq!(packages, BTreeSet::from(["serde".to_string()]));
        let uv = parse(
            Format::Uv,
            "version = 1\n\n[[package]]\nname = \"requests\"\nversion = \"2.32.0\"\n",
        )
        .unwrap();
        assert_eq!(uv, BTreeSet::from(["requests".to_string()]));
        let poetry = parse(
            Format::Poetry,
            "[[package]]\nname = \"requests\"\nversion = \"2.32.0\"\n\n[metadata]\nlock-version = \"2.0\"\n",
        )
        .unwrap();
        assert_eq!(poetry, BTreeSet::from(["requests".to_string()]));
    }

    #[test]
    fn pipfile_lock_sections_are_interpreted() {
        let packages = parse(
            Format::Pipfile,
            r#"{"_meta":{"hash":{"sha256":"x"}},"default":{"requests":{"version":"==2.32.0"}},"develop":{"pytest":{"version":"==8.0.0"}}}"#,
        )
        .unwrap();
        assert_eq!(
            packages,
            BTreeSet::from(["pytest".to_string(), "requests".to_string()])
        );
    }
}
