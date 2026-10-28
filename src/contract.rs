//! The canonical PAX observation contract.
//!
//! Every JSON document PAX emits carries a canonical `schema` identifier
//! (`pax.<subject>.v<major>`). Observation documents additionally carry an
//! `outcome` describing how far PAX got, and every fact carries evidence that
//! names its source.
//!
//! The rules here are deliberately small and explicit. They exist so callers can
//! distinguish "I looked and it is absent" from "I did not look", "this is
//! unsupported", and "the observation failed". See `docs/PAX_CONTRACT.md`.
//!
//! This module defines the contract; it does not implement new observers.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

/// Canonical identifier for this contract document.
pub const CONTRACT_SCHEMA: &str = "pax.contract.v1";

/// Existing project-structure observation schema. Retained unchanged.
pub const OBSERVATION_SCHEMA: &str = "pax.observation.v1";

/// Existing delegated test-result schema. Retained unchanged.
pub const EXECUTION_RESULT_SCHEMA: &str = "pax.execution-result.v1";

/// Dry-run execution plan schema.
pub const PLAN_SCHEMA: &str = "pax.plan.v1";

/// Inspection document schemas.
pub const INFO_SCHEMA: &str = "pax.info.v1";
pub const DOCTOR_SCHEMA: &str = "pax.doctor.v1";
pub const DEPS_SCHEMA: &str = "pax.deps.v1";
pub const SCRIPTS_SCHEMA: &str = "pax.scripts.v1";
pub const WORKSPACES_SCHEMA: &str = "pax.workspaces.v1";
pub const LOCK_SCHEMA: &str = "pax.lock.v1";
pub const GRAPH_SCHEMA: &str = "pax.graph.v1";
pub const REALITY_SCHEMA: &str = "pax.reality.v1";
pub const DRIFT_SCHEMA: &str = "pax.drift.v1";

/// Legacy numeric schema version, retained under `schemaVersion` /
/// `schema_version` so existing consumers keep working.
pub const LEGACY_SCHEMA_VERSION: &str = "1";

/// Canonical schema identifier for an inspection command.
pub fn inspection_schema(command: &str) -> &'static str {
    match command {
        "info" => INFO_SCHEMA,
        "doctor" => DOCTOR_SCHEMA,
        "deps" => DEPS_SCHEMA,
        "scripts" => SCRIPTS_SCHEMA,
        "workspaces" => WORKSPACES_SCHEMA,
        "lock" => LOCK_SCHEMA,
        "graph" => GRAPH_SCHEMA,
        "reality" => REALITY_SCHEMA,
        "drift" => DRIFT_SCHEMA,
        _ => CONTRACT_SCHEMA,
    }
}

/// How far an observation got.
///
/// `Ok` is the only successful outcome. `Partial` means PAX established some
/// facts and explicitly could not establish others. An unsupported observation
/// is never reported as `Ok`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The observation completed and every subject it reports was established.
    Ok,
    /// Some facts were established; at least one subject was not.
    Partial,
    /// PAX does not know how to observe the requested subject.
    Unsupported,
    /// A bound, precondition, or required input prevented the observation.
    Blocked,
    /// PAX attempted the observation and it did not complete.
    Failed,
    /// The observation describes a state older than the current subject.
    Stale,
}

impl Outcome {
    pub const ALL: [Self; 6] = [
        Self::Ok,
        Self::Partial,
        Self::Unsupported,
        Self::Blocked,
        Self::Failed,
        Self::Stale,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }

    /// Only `Ok` is success. `Partial` is explicitly not success.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// The state of a single fact or subject, distinct from the outcome of the
/// whole observation.
///
/// `Absent` is a successful observation of non-existence. `Unknown` means PAX
/// did not establish the fact. `Unsupported` means PAX has no observer for it.
/// `Blocked` means a bound or precondition prevented it. `Failed` means an
/// attempt did not complete. `Stale` means the fact is older than its subject.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FactState {
    Present,
    Absent,
    Unknown,
    Unsupported,
    Blocked,
    Failed,
    Stale,
}

impl FactState {
    pub const ALL: [Self; 7] = [
        Self::Present,
        Self::Absent,
        Self::Unknown,
        Self::Unsupported,
        Self::Blocked,
        Self::Failed,
        Self::Stale,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }
}

/// How strongly evidence supports a fact.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceStrength {
    /// A project artifact states it; nothing checked it.
    Declared,
    /// PAX read the artifact on this machine at observation time.
    Observed,
    /// A resolver established it.
    Resolved,
    /// Execution established it. Observation never emits this strength.
    Verified,
}

impl EvidenceStrength {
    pub const ALL: [Self; 4] = [
        Self::Declared,
        Self::Observed,
        Self::Resolved,
        Self::Verified,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Observed => "observed",
            Self::Resolved => "resolved",
            Self::Verified => "verified",
        }
    }
}

/// A stable, machine-independent identity for an entity in a project.
///
/// Identities are relative to the project root. The root is `.`; paths use `/`.
/// Absolute paths, drive prefixes, and `..` traversal are never identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Entity {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

impl Entity {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            attributes: BTreeMap::new(),
        }
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Whether this entity's id obeys the project-relative identity rule.
    pub fn has_canonical_id(&self) -> bool {
        is_project_relative_identity(&self.id)
    }
}

/// One piece of evidence for a fact, traceable to a source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Evidence {
    /// Project-relative file path, or the name of a tool/runtime source.
    pub source: String,
    /// How the evidence was obtained, e.g. `fs.stat`, `cargo.metadata`.
    pub method: String,
    pub strength: EvidenceStrength,
    /// Optional finer locator: a line number, field, or command selector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

impl Evidence {
    pub fn new(
        source: impl Into<String>,
        method: impl Into<String>,
        strength: EvidenceStrength,
    ) -> Self {
        Self {
            source: source.into(),
            method: method.into(),
            strength,
            locator: None,
        }
    }

    pub fn at(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }
}

/// A canonical statement that PAX could not establish something.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservationDiagnostic {
    pub code: String,
    pub state: FactState,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}

/// A reason a string cannot be a canonical project-relative identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityError {
    pub value: String,
    pub reason: &'static str,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid project-relative identity {:?}: {}",
            self.value, self.reason
        )
    }
}

impl std::error::Error for IdentityError {}

/// Normalizes a path to a canonical project-relative identity.
///
/// `/` is the separator, `.` is the project root, empty and `.` components are
/// removed, and `..` is rejected rather than resolved. Absolute paths, Windows
/// drive prefixes, and UNC paths are rejected.
pub fn project_relative_path(raw: &str) -> Result<String, IdentityError> {
    if raw.is_empty() {
        return Ok(".".to_string());
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(IdentityError {
            value: raw.to_string(),
            reason: "absolute paths are not project-relative identities",
        });
    }
    if has_drive_prefix(&normalized) {
        return Err(IdentityError {
            value: raw.to_string(),
            reason: "drive-prefixed paths are not project-relative identities",
        });
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(IdentityError {
                    value: raw.to_string(),
                    reason: "parent traversal is not a project-relative identity",
                });
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(parts.join("/"))
    }
}

/// Whether `raw` can be used as a canonical, machine-independent identity.
///
/// This is broader than [`project_relative_path`] because entity ids may carry
/// selectors (`crate::module`, `path::item`) and package names. It enforces the
/// invariant that matters: no absolute path and no `..` traversal.
pub fn is_project_relative_identity(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') || has_drive_prefix(&normalized) {
        return false;
    }
    !normalized.split('/').any(|part| part == "..")
}

fn has_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_ids_are_versioned_and_namespaced() {
        for schema in [
            CONTRACT_SCHEMA,
            OBSERVATION_SCHEMA,
            EXECUTION_RESULT_SCHEMA,
            PLAN_SCHEMA,
            INFO_SCHEMA,
            DOCTOR_SCHEMA,
            DEPS_SCHEMA,
            SCRIPTS_SCHEMA,
            WORKSPACES_SCHEMA,
            LOCK_SCHEMA,
            GRAPH_SCHEMA,
            REALITY_SCHEMA,
            DRIFT_SCHEMA,
        ] {
            assert!(schema.starts_with("pax."), "{schema}");
            assert!(schema.ends_with(".v1"), "{schema}");
        }
        assert_eq!(inspection_schema("info"), INFO_SCHEMA);
        assert_eq!(inspection_schema("graph"), GRAPH_SCHEMA);
    }

    #[test]
    fn outcome_vocabulary_is_stable_and_only_ok_is_success() {
        let serialized = Outcome::ALL
            .iter()
            .map(|outcome| serde_json::to_string(outcome).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            serialized,
            [
                "\"ok\"",
                "\"partial\"",
                "\"unsupported\"",
                "\"blocked\"",
                "\"failed\"",
                "\"stale\""
            ]
        );
        assert!(Outcome::Ok.is_success());
        for outcome in Outcome::ALL.into_iter().filter(|o| *o != Outcome::Ok) {
            assert!(!outcome.is_success(), "{outcome:?}");
        }
    }

    #[test]
    fn fact_states_keep_absence_ignorance_and_unsupported_distinct() {
        let serialized = FactState::ALL
            .iter()
            .map(|state| serde_json::to_string(state).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            serialized,
            [
                "\"present\"",
                "\"absent\"",
                "\"unknown\"",
                "\"unsupported\"",
                "\"blocked\"",
                "\"failed\"",
                "\"stale\""
            ]
        );
        let distinct = FactState::ALL
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct.len(), FactState::ALL.len());
    }

    #[test]
    fn evidence_serializes_a_traceable_source() {
        let evidence =
            Evidence::new("Cargo.toml", "fs.stat", EvidenceStrength::Observed).at("line 1");
        let value = serde_json::to_value(evidence).unwrap();
        assert_eq!(value["source"], "Cargo.toml");
        assert_eq!(value["method"], "fs.stat");
        assert_eq!(value["strength"], "observed");
        assert_eq!(value["locator"], "line 1");
    }

    #[test]
    fn identities_are_project_relative() {
        assert_eq!(project_relative_path("").unwrap(), ".");
        assert_eq!(project_relative_path(".").unwrap(), ".");
        assert_eq!(project_relative_path("./src/a.rs").unwrap(), "src/a.rs");
        assert_eq!(project_relative_path("src\\a.rs").unwrap(), "src/a.rs");
        for rejected in ["/tmp/project", "C:\\project", "src/../secrets", ".."] {
            assert!(project_relative_path(rejected).is_err(), "{rejected}");
        }
        assert!(is_project_relative_identity("."));
        assert!(is_project_relative_identity("src/a.rs"));
        assert!(is_project_relative_identity("pax/lib::crate::a"));
        assert!(is_project_relative_identity("@scope/pkg"));
        assert!(!is_project_relative_identity("/tmp/project"));
        assert!(!is_project_relative_identity("C:\\project"));
        assert!(!is_project_relative_identity("../project"));
    }

    #[test]
    fn entities_validate_their_ids() {
        let entity = Entity::new("component", "src/app").with("ecosystem", "rust");
        assert!(entity.has_canonical_id());
        assert_eq!(
            serde_json::to_value(entity).unwrap(),
            serde_json::json!({
                "type": "component",
                "id": "src/app",
                "attributes": {"ecosystem": "rust"}
            })
        );
        assert!(!Entity::new("component", "/tmp/app").has_canonical_id());
    }
}
