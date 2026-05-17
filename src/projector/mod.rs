use std::collections::BTreeMap;

use crate::Result;

#[cfg(feature = "graph-kuzu")]
pub mod kuzu;

pub type GraphProperties = BTreeMap<String, GraphValue>;
pub type GraphParams = BTreeMap<String, GraphValue>;
pub type GraphRow = BTreeMap<String, GraphValue>;

#[derive(Clone, Debug, PartialEq)]
pub enum GraphValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringList(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    Decision,
    Actor,
    Evidence,
    Option,
    Hypothesis,
}

impl NodeKind {
    pub const ALL: [Self; 5] = [
        Self::Decision,
        Self::Actor,
        Self::Evidence,
        Self::Option,
        Self::Hypothesis,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::Actor => "Actor",
            Self::Evidence => "Evidence",
            Self::Option => "Option",
            Self::Hypothesis => "Hypothesis",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    ProposedBy,
    AcceptedBy,
    RejectedBy,
    Supersedes,
    BasedOn,
    HasOption,
    Chose,
    Assumes,
    Supports,
    Refutes,
}

impl RelationKind {
    pub const ALL: [Self; 10] = [
        Self::ProposedBy,
        Self::AcceptedBy,
        Self::RejectedBy,
        Self::Supersedes,
        Self::BasedOn,
        Self::HasOption,
        Self::Chose,
        Self::Assumes,
        Self::Supports,
        Self::Refutes,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::ProposedBy => "PROPOSED_BY",
            Self::AcceptedBy => "ACCEPTED_BY",
            Self::RejectedBy => "REJECTED_BY",
            Self::Supersedes => "SUPERSEDES",
            Self::BasedOn => "BASED_ON",
            Self::HasOption => "HAS_OPTION",
            Self::Chose => "CHOSE",
            Self::Assumes => "ASSUMES",
            Self::Supports => "SUPPORTS",
            Self::Refutes => "REFUTES",
        }
    }

    pub const fn endpoints(self) -> (NodeKind, NodeKind) {
        match self {
            Self::ProposedBy | Self::AcceptedBy | Self::RejectedBy => {
                (NodeKind::Decision, NodeKind::Actor)
            }
            Self::Supersedes => (NodeKind::Decision, NodeKind::Decision),
            Self::BasedOn => (NodeKind::Decision, NodeKind::Evidence),
            Self::HasOption | Self::Chose => (NodeKind::Decision, NodeKind::Option),
            Self::Assumes => (NodeKind::Decision, NodeKind::Hypothesis),
            Self::Supports | Self::Refutes => (NodeKind::Evidence, NodeKind::Hypothesis),
        }
    }
}

pub trait GraphView {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()>;

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()>;

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>>;

    fn wipe(&self) -> Result<()>;
}
