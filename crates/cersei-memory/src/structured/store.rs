//! Grafeo access for the structured memory.
//!
//! **Reads** are fixed GQL texts (`q::*`) whose values — content, names,
//! quotes, ids, times — travel as typed parameters; they start from a node
//! found through a property index (`{id: $id}`, `{subject_id: …}`,
//! `{alias_key: …}`), which the Grafeo 0.5.43 planner serves in constant
//! time, while a labelled pattern (`(:Episode {id: $id})`) scans the label.
//!
//! **Writes** are typed operations ([`W`]) applied through Grafeo's node and
//! edge API inside one transaction: no query text at all, so nothing a user
//! or a model wrote can be read as query syntax. (Measured on 0.5.43: GQL
//! `INSERT` / `SET` / `MATCH … INSERT` grow superlinearly with the store —
//! about 2.5 ms per edge at 4 000 nodes — the typed API stays at a few
//! microseconds; see `docs/memory.md`.)
//!
//! Nothing a user or a model wrote is ever spliced into a query.
//!
//! ```text
//! (:Episode {id, space, session_id, role, author, content, occurred_at,
//!            recorded_at, source_ref, extraction_state, extraction_attempts,
//!            extraction_json, extraction_error, embedding, embed_model, ikey})
//! (:Entity  {id, space, type, name, name_norm, aliases})
//! (:Fact    {id, space, subject_id, subject_name, predicate, value,
//!            value_norm, object_id, negated, origin, status, confidence,
//!            valid_from, valid_until, asserted_at, superseded_until, ended_by,
//!            recorded_at, superseded_at, retracted_at, statement, embedding,
//!            embed_model, ikey})
//! (:Fact)-[:SUPPORTED_BY {quote}]->(:Episode)
//! (:Fact)-[:ABOUT]->(:Entity)            subject
//! (:Fact)-[:MENTIONS]->(:Entity)         object, when it is an entity
//! (:Episode)-[:MENTIONS]->(:Entity)
//! (:Fact)-[:SUPERSEDES {at}]->(:Fact)    new version → the one it replaces
//! (:Fact)-[:CONTRADICTS]->(:Fact)        unresolved conflict (both kept)
//! (:Alias {alias_key, entity_id})        alias_key = space ␟ normalised name
//! (:MemoryMeta {id: 'meta:memory', singleton, embed_model, embed_dims})
//! ```
//!
//! Times are `Int64` milliseconds (UTC); an absent time is an unknown time.

use super::model::*;
use grafeo::{GrafeoDB, NodeId, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

pub type StoreResult<T> = Result<T, String>;

/// Fixed read queries.
///
/// A query that starts from an indexed property has no `WHERE`: in Grafeo
/// 0.5.43 a `WHERE` clause makes the planner scan instead of using the
/// property index (measured: 80 µs → 17 ms at 4 000 nodes). Such queries
/// return the properties and the filter is applied in Rust.
pub mod q {
    pub const EPISODE_EXISTS: &str = "MATCH (e {id: $id}) RETURN e.id";
    pub const EPISODE: &str = "MATCH (e {id: $id}) RETURN e.id, e.space, e.session_id, \
        e.role, e.author, e.content, e.occurred_at, e.recorded_at, e.source_ref";
    pub const EPISODE_STATE: &str = "MATCH (e {id: $id}) RETURN e.extraction_state, \
        e.extraction_attempts, e.extraction_json";
    pub const PENDING_EPISODES: &str = "MATCH (e:Episode) WHERE e.extraction_state IN $states \
        RETURN e.id ORDER BY e.recorded_at, e.id";
    pub const EPISODES_OF_SESSION: &str =
        "MATCH (e {session_id: $session_id}) RETURN e.id, e.ikey ORDER BY e.id";

    pub const ALIAS: &str = "MATCH (a {alias_key: $key}) RETURN a.entity_id ORDER BY a.entity_id";
    pub const ENTITY: &str = "MATCH (n {id: $id}) RETURN n.name, n.type, n.aliases";

    /// Facts about a subject (filtered by [`super::Store::facts_by_key`]).
    pub const FACTS_OF_SUBJECT: &str = "MATCH (f {subject_id: $subject}) \
        RETURN f.id, f.space, f.predicate, f.status, f.recorded_at";
    pub const ALL_FACTS: &str = "MATCH (f:Fact) RETURN f.id, f.space, f.subject_id, f.subject_name, \
        f.predicate, f.value, f.value_norm, f.object_id, f.negated, f.origin, f.status, f.confidence, \
        f.valid_from, f.valid_until, f.asserted_at, f.superseded_until, f.ended_by, f.recorded_at, \
        f.superseded_at, f.retracted_at, f.statement ORDER BY f.id";
    pub const ALL_EPISODES: &str = "MATCH (e:Episode) RETURN e.id, e.space, e.session_id, \
        e.role, e.author, e.content, e.occurred_at, e.recorded_at, e.source_ref ORDER BY e.id";
    pub const FACT: &str = "MATCH (f {id: $id}) RETURN f.id, f.space, f.subject_id, f.subject_name, \
        f.predicate, f.value, f.value_norm, f.object_id, f.negated, f.origin, f.status, f.confidence, \
        f.valid_from, f.valid_until, f.asserted_at, f.superseded_until, f.ended_by, f.recorded_at, \
        f.superseded_at, f.retracted_at, f.statement";
    pub const FACT_EVIDENCE: &str = "MATCH (f {id: $fact})-[s:SUPPORTED_BY]->(e) \
        RETURN e.id, s.quote, e.session_id, e.role, e.occurred_at ORDER BY e.recorded_at, e.id";
    pub const SUPERSEDED_BY: &str =
        "MATCH (o {id: $id})<-[:SUPERSEDES]-(n) RETURN n.id ORDER BY n.id";
    pub const SUPERSEDES: &str = "MATCH (n {id: $id})-[:SUPERSEDES]->(o) RETURN o.id ORDER BY o.id";
    pub const CONTRADICTIONS: &str =
        "MATCH (a {id: $id})-[:CONTRADICTS]-(b) RETURN b.id ORDER BY b.id";
    pub const FACTS_OF_SESSION: &str = "MATCH (e {session_id: $session_id})<-[:SUPPORTED_BY]-(f) \
        RETURN DISTINCT f.id ORDER BY f.id";
    pub const EVIDENCE_COUNT: &str = "MATCH (f {id: $fact})-[s:SUPPORTED_BY]->(e) RETURN count(s)";

    pub const INDEXED_ITEMS: &str =
        "MATCH (n) WHERE n.ikey IS NOT NULL AND n.embed_model = $model \
        RETURN n.ikey, n.id, labels(n), n.embedding";
    pub const IKEY: &str = "MATCH (n {id: $id}) RETURN n.ikey";
    pub const MAX_IKEY: &str = "MATCH (n) WHERE n.ikey IS NOT NULL RETURN max(n.ikey)";
    pub const UNEMBEDDED: &str = "MATCH (n) WHERE n.ikey IS NOT NULL AND n.embed_model <> $model \
        RETURN n.id, labels(n) ORDER BY n.id";

    pub const META: &str = "MATCH (m {singleton: 'meta'}) RETURN m.embed_model, m.embed_dims";

    pub const COUNT_EPISODES: &str = "MATCH (e:Episode) RETURN count(e)";
    pub const COUNT_FACTS: &str = "MATCH (f:Fact) RETURN count(f)";
    pub const COUNT_ENTITIES: &str = "MATCH (n:Entity) RETURN count(n)";

    // Bulk writes that stay single GQL statements.
    pub(super) const DELETE_NODE: &str = "MATCH (n {id: $id}) DETACH DELETE n";
    pub(super) const CLEAR_EMBEDDINGS: &str =
        "MATCH (n) WHERE n.ikey IS NOT NULL SET n.embed_model = ''";
}

/// Typed write operations. The parameters (`P`) of each are listed with it;
/// node operations find their node by its `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum W {
    /// `id space session_id role author content occurred_at recorded_at source_ref ikey`
    InsertEpisode,
    /// `id state attempts json error`
    SetExtraction,
    /// `id embedding model`
    SetEmbedding,
    /// `id space type name norm aliases`
    InsertEntity,
    /// `key entity_id`
    InsertAlias,
    /// `id aliases`
    SetEntityAliases,
    /// `episode entity`
    LinkEpisodeEntity,
    /// `id space subject_id subject_name predicate value value_norm object_id negated origin
    /// status confidence valid_from valid_until asserted_at recorded_at statement ikey`
    InsertFact,
    /// `id status`
    SetFactStatus,
    /// `id at until ended_by`
    SupersedeFact,
    /// `id at`
    RetractFact,
    /// `fact episode quote`
    LinkFactEvidence,
    /// `fact entity`
    LinkFactAbout,
    /// `fact entity`
    LinkFactMentions,
    /// `new old at`
    LinkSupersedes,
    /// `a b`
    LinkContradicts,
    /// `model dims`
    InsertMeta,
    /// `model dims`
    SetMeta,
    /// `id`
    DeleteNode,
    /// (no parameter)
    ClearEmbeddings,
}

/// The key of an alias node: the space and the normalised name.
pub fn alias_key(space: &str, norm: &str) -> String {
    format!("{space}\u{1f}{norm}")
}

/// Parameters of a query.
pub struct P(HashMap<String, Value>);

impl P {
    pub fn new() -> Self {
        Self(HashMap::new())
    }
    pub fn s(mut self, k: &str, v: &str) -> Self {
        self.0.insert(k.into(), Value::from(v));
        self
    }
    pub fn os(mut self, k: &str, v: Option<&str>) -> Self {
        self.0
            .insert(k.into(), v.map(Value::from).unwrap_or(Value::Null));
        self
    }
    pub fn i(mut self, k: &str, v: i64) -> Self {
        self.0.insert(k.into(), Value::Int64(v));
        self
    }
    pub fn oi(mut self, k: &str, v: Option<i64>) -> Self {
        self.0
            .insert(k.into(), v.map(Value::Int64).unwrap_or(Value::Null));
        self
    }
    pub fn of(mut self, k: &str, v: Option<f64>) -> Self {
        self.0
            .insert(k.into(), v.map(Value::Float64).unwrap_or(Value::Null));
        self
    }
    pub fn b(mut self, k: &str, v: bool) -> Self {
        self.0.insert(k.into(), Value::Bool(v));
        self
    }
    pub fn list(mut self, k: &str, v: &[String]) -> Self {
        let items: Vec<Value> = v.iter().map(|s| Value::from(s.as_str())).collect();
        self.0.insert(k.into(), Value::List(items.into()));
        self
    }
    pub fn vector(mut self, k: &str, v: &[f32]) -> Self {
        self.0.insert(k.into(), Value::Vector(v.to_vec().into()));
        self
    }
}

impl P {
    fn take(&mut self, k: &str) -> Value {
        self.0.remove(k).unwrap_or(Value::Null)
    }

    fn take_str(&mut self, k: &str) -> StoreResult<String> {
        as_str(&self.take(k)).ok_or_else(|| format!("missing `{k}`"))
    }
}

impl Default for P {
    fn default() -> Self {
        Self::new()
    }
}

/// Value readers.
pub fn as_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.to_string()),
        _ => None,
    }
}
pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int64(n) => Some(*n),
        Value::Float64(f) => Some(*f as i64),
        _ => None,
    }
}
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float64(f) => Some(*f),
        Value::Int64(n) => Some(*n as f64),
        _ => None,
    }
}
pub fn as_bool(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}
pub fn as_strings(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => items.iter().filter_map(as_str).collect(),
        _ => Vec::new(),
    }
}
pub fn as_vector(v: &Value) -> Option<Vec<f32>> {
    match v {
        Value::Vector(x) => Some(x.to_vec()),
        Value::List(items) => items.iter().map(|i| as_f64(i).map(|f| f as f32)).collect(),
        _ => None,
    }
}

/// The Grafeo database of the memory.
pub struct Store {
    db: GrafeoDB,
    next_ikey: AtomicI64,
    /// Text indexes created so far (Grafeo refuses one on an empty label).
    text_indexes: parking_lot::Mutex<HashMap<&'static str, bool>>,
    /// Typed writes do not update Grafeo's text indexes: they are rebuilt
    /// before the next lexical search.
    dirty_text: AtomicBool,
    /// Held for writing across a whole transaction, for reading by the
    /// reads that bypass Grafeo's snapshots (adjacency lists, single
    /// properties, text indexes): they never observe a transaction that has
    /// not committed. Only local work happens under it — never a model or
    /// embedding call.
    commit_lock: parking_lot::RwLock<()>,
}

impl Store {
    pub fn open(path: &Path) -> StoreResult<Self> {
        let db = GrafeoDB::open(path).map_err(|e| format!("cannot open the memory store: {e}"))?;
        Self::init(db)
    }

    pub fn in_memory() -> StoreResult<Self> {
        Self::init(GrafeoDB::new_in_memory())
    }

    /// Properties looked up by equality: indexed so a lookup does not scan
    /// every node. Grafeo keeps them up to date; they are rebuilt at open.
    pub const INDEXED_PROPERTIES: [&'static str; 5] =
        ["id", "session_id", "subject_id", "alias_key", "singleton"];

    fn init(db: GrafeoDB) -> StoreResult<Self> {
        for p in Self::INDEXED_PROPERTIES {
            db.create_property_index(p);
        }
        match crate::graph_migrate::check_version(&db) {
            crate::graph_migrate::VersionCheck::UpToDate => {}
            crate::graph_migrate::VersionCheck::NeedsMigration { from, to } => {
                crate::graph_migrate::run_migrations(&db, from, to).map_err(|e| e.to_string())?
            }
            crate::graph_migrate::VersionCheck::CodeBehind {
                graph_version,
                code_version,
            } => {
                return Err(format!(
                    "the memory store has schema v{graph_version}, newer than this code (v{code_version})"
                ))
            }
        }
        let s = Self {
            db,
            next_ikey: AtomicI64::new(1),
            text_indexes: parking_lot::Mutex::new(HashMap::new()),
            dirty_text: AtomicBool::new(true),
            commit_lock: parking_lot::RwLock::new(()),
        };
        let max = s.query(q::MAX_IKEY, P::new())?;
        let max = max
            .first()
            .and_then(|r| r.first())
            .and_then(as_i64)
            .unwrap_or(0);
        s.next_ikey.store(max + 1, Ordering::SeqCst);
        Ok(s)
    }

    pub fn db(&self) -> &GrafeoDB {
        &self.db
    }

    pub fn next_ikey(&self) -> i64 {
        self.next_ikey.fetch_add(1, Ordering::SeqCst)
    }

    /// Run a fixed query; rows as value vectors.
    pub fn query(&self, query: &'static str, p: P) -> StoreResult<Vec<Vec<Value>>> {
        let s = self.db.session();
        let r = s
            .execute_with_params(query, p.0)
            .map_err(|e| format!("memory query failed: {e}"))?;
        Ok(r.iter().map(|row| row.to_vec()).collect())
    }

    /// Apply typed writes in one transaction: all or none.
    pub fn transaction(&self, ops: Vec<(W, P)>) -> StoreResult<()> {
        let _commit = self.commit_lock.write();
        let mut s = self.db.session();
        s.begin_transaction()
            .map_err(|e| format!("cannot begin a memory transaction: {e}"))?;
        let mut local: HashMap<String, NodeId> = HashMap::new();
        for (w, p) in ops {
            if let Err(e) = self.apply(&s, &mut local, w, p) {
                let _ = s.rollback();
                return Err(format!("memory transaction rolled back: {e}"));
            }
        }
        #[cfg(test)]
        if let Some(hook) = tests::BEFORE_COMMIT.lock().take() {
            if let Err(e) = hook() {
                let _ = s.rollback();
                return Err(format!("memory transaction rolled back: {e}"));
            }
        }
        s.commit()
            .map_err(|e| format!("memory transaction failed to commit: {e}"))?;
        self.dirty_text.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// One typed write (its own transaction).
    pub fn exec(&self, w: W, p: P) -> StoreResult<()> {
        self.transaction(vec![(w, p)])
    }

    fn node(&self, local: &HashMap<String, NodeId>, id: &str) -> StoreResult<NodeId> {
        if let Some(n) = local.get(id) {
            return Ok(*n);
        }
        self.db
            .find_nodes_by_property("id", &Value::from(id))
            .into_iter()
            .next()
            .ok_or_else(|| format!("no memory record `{id}`"))
    }

    fn apply(
        &self,
        s: &grafeo::Session,
        local: &mut HashMap<String, NodeId>,
        w: W,
        mut p: P,
    ) -> StoreResult<()> {
        let err = |e: grafeo::Error| e.to_string();
        let create = |s: &grafeo::Session,
                      local: &mut HashMap<String, NodeId>,
                      label: &str,
                      props: Vec<(&'static str, Value)>|
         -> StoreResult<()> {
            let id = props
                .iter()
                .find(|(k, _)| *k == "id")
                .and_then(|(_, v)| as_str(v));
            let nid = s.create_node_with_props(&[label], props).map_err(err)?;
            if let Some(id) = id {
                local.insert(id, nid);
            }
            Ok(())
        };
        let set = |s: &grafeo::Session,
                   nid: NodeId,
                   props: Vec<(&'static str, Value)>|
         -> StoreResult<()> {
            for (k, v) in props {
                s.set_node_property(nid, k, v).map_err(err)?;
            }
            Ok(())
        };
        let edge = |s: &grafeo::Session,
                    this: &Store,
                    local: &HashMap<String, NodeId>,
                    from: &str,
                    to: &str,
                    kind: &str,
                    props: Vec<(&'static str, Value)>|
         -> StoreResult<()> {
            let a = this.node(local, from)?;
            let b = this.node(local, to)?;
            s.create_edge_with_props(a, b, kind, props).map_err(err)?;
            Ok(())
        };
        match w {
            W::InsertEpisode => {
                let mut props: Vec<(&'static str, Value)> = Vec::new();
                for k in [
                    "id",
                    "space",
                    "session_id",
                    "role",
                    "author",
                    "content",
                    "occurred_at",
                    "recorded_at",
                    "source_ref",
                    "ikey",
                ] {
                    props.push((k, p.take(k)));
                }
                props.push(("extraction_state", Value::from("pending")));
                props.push(("extraction_attempts", Value::Int64(0)));
                props.push(("embed_model", Value::from("")));
                create(s, local, "Episode", props)
            }
            W::SetExtraction => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(
                    s,
                    nid,
                    vec![
                        ("extraction_state", p.take("state")),
                        ("extraction_attempts", p.take("attempts")),
                        ("extraction_json", p.take("json")),
                        ("extraction_error", p.take("error")),
                    ],
                )
            }
            W::SetEmbedding => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(
                    s,
                    nid,
                    vec![
                        ("embedding", p.take("embedding")),
                        ("embed_model", p.take("model")),
                    ],
                )
            }
            W::InsertEntity => {
                let props = vec![
                    ("id", p.take("id")),
                    ("space", p.take("space")),
                    ("type", p.take("type")),
                    ("name", p.take("name")),
                    ("name_norm", p.take("norm")),
                    ("aliases", p.take("aliases")),
                ];
                create(s, local, "Entity", props)
            }
            W::InsertAlias => {
                let props = vec![
                    ("alias_key", p.take("key")),
                    ("entity_id", p.take("entity_id")),
                ];
                create(s, local, "Alias", props)
            }
            W::SetEntityAliases => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(s, nid, vec![("aliases", p.take("aliases"))])
            }
            W::LinkEpisodeEntity => {
                let (a, b) = (p.take_str("episode")?, p.take_str("entity")?);
                edge(s, self, local, &a, &b, "MENTIONS", Vec::new())
            }
            W::InsertFact => {
                let mut props: Vec<(&'static str, Value)> = Vec::new();
                for k in [
                    "id",
                    "space",
                    "subject_id",
                    "subject_name",
                    "predicate",
                    "value",
                    "value_norm",
                    "object_id",
                    "negated",
                    "origin",
                    "status",
                    "confidence",
                    "valid_from",
                    "valid_until",
                    "asserted_at",
                    "recorded_at",
                    "statement",
                    "ikey",
                ] {
                    props.push((k, p.take(k)));
                }
                props.push(("embed_model", Value::from("")));
                create(s, local, "Fact", props)
            }
            W::SetFactStatus => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(s, nid, vec![("status", p.take("status"))])
            }
            W::SupersedeFact => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(
                    s,
                    nid,
                    vec![
                        ("status", Value::from("superseded")),
                        ("superseded_at", p.take("at")),
                        ("superseded_until", p.take("until")),
                        ("ended_by", p.take("ended_by")),
                    ],
                )
            }
            W::RetractFact => {
                let nid = self.node(local, &p.take_str("id")?)?;
                set(
                    s,
                    nid,
                    vec![
                        ("status", Value::from("retracted")),
                        ("retracted_at", p.take("at")),
                    ],
                )
            }
            W::LinkFactEvidence => {
                let (a, b) = (p.take_str("fact")?, p.take_str("episode")?);
                edge(
                    s,
                    self,
                    local,
                    &a,
                    &b,
                    "SUPPORTED_BY",
                    vec![("quote", p.take("quote"))],
                )
            }
            W::LinkFactAbout => {
                let (a, b) = (p.take_str("fact")?, p.take_str("entity")?);
                edge(s, self, local, &a, &b, "ABOUT", Vec::new())
            }
            W::LinkFactMentions => {
                let (a, b) = (p.take_str("fact")?, p.take_str("entity")?);
                edge(s, self, local, &a, &b, "MENTIONS", Vec::new())
            }
            W::LinkSupersedes => {
                let (a, b) = (p.take_str("new")?, p.take_str("old")?);
                edge(
                    s,
                    self,
                    local,
                    &a,
                    &b,
                    "SUPERSEDES",
                    vec![("at", p.take("at"))],
                )
            }
            W::LinkContradicts => {
                let (a, b) = (p.take_str("a")?, p.take_str("b")?);
                edge(s, self, local, &a, &b, "CONTRADICTS", Vec::new())
            }
            W::InsertMeta => create(
                s,
                local,
                "MemoryMeta",
                vec![
                    ("id", Value::from("meta:memory")),
                    ("singleton", Value::from("meta")),
                    ("embed_model", p.take("model")),
                    ("embed_dims", p.take("dims")),
                ],
            ),
            W::SetMeta => {
                let nid = self.node(local, "meta:memory")?;
                set(
                    s,
                    nid,
                    vec![
                        ("embed_model", p.take("model")),
                        ("embed_dims", p.take("dims")),
                    ],
                )
            }
            W::DeleteNode => s
                .execute_with_params(q::DELETE_NODE, p.0)
                .map(|_| ())
                .map_err(err),
            W::ClearEmbeddings => s.execute(q::CLEAR_EMBEDDINGS).map(|_| ()).map_err(err),
        }
    }

    pub fn count(&self, query: &'static str) -> usize {
        self.query(query, P::new())
            .ok()
            .and_then(|r| r.first().and_then(|row| row.first()).and_then(as_i64))
            .unwrap_or(0) as usize
    }

    /// BM25 search over `label.property`, ids with scores. The text index
    /// is created on first use (Grafeo refuses one on an empty label) and
    /// kept in sync by Grafeo afterwards; it is rebuilt after a reopen.
    pub fn text_search(
        &self,
        label: &'static str,
        property: &'static str,
        query: &str,
        k: usize,
    ) -> Vec<(String, f64)> {
        let key = if label == "Fact" { "Fact" } else { "Episode" };
        let _committed = self.commit_lock.read();
        {
            let mut idx = self.text_indexes.lock();
            if self.dirty_text.swap(false, Ordering::SeqCst) {
                // Written since the last search: rebuild what exists.
                idx.clear();
            }
            if !idx.get(key).copied().unwrap_or(false) {
                let _ = self.db.drop_text_index(label, property);
                let ok = self.db.create_text_index(label, property).is_ok();
                idx.insert(key, ok);
                if !ok {
                    return Vec::new();
                }
            }
        }
        let Ok(hits) = self.db.text_search(label, property, query, k) else {
            return Vec::new();
        };
        let lpg = self.db.store();
        hits.into_iter()
            .filter_map(|(node, score)| {
                lpg.get_node_property(node, &"id".into())
                    .as_ref()
                    .and_then(as_str)
                    .map(|id| (id, score))
            })
            .collect()
    }

    /// Open facts with this space, subject and predicate, oldest first.
    pub fn facts_by_key(
        &self,
        space: &str,
        subject: &str,
        predicate: &str,
        statuses: &[&str],
    ) -> StoreResult<Vec<String>> {
        let mut rows: Vec<(i64, String)> = self
            .query(q::FACTS_OF_SUBJECT, P::new().s("subject", subject))?
            .iter()
            .filter(|r| {
                as_str(&r[1]).as_deref() == Some(space)
                    && as_str(&r[2]).as_deref() == Some(predicate)
                    && as_str(&r[3]).is_some_and(|st| statuses.contains(&st.as_str()))
            })
            .filter_map(|r| Some((as_i64(&r[4]).unwrap_or(0), as_str(&r[0])?)))
            .collect();
        rows.sort();
        Ok(rows.into_iter().map(|(_, id)| id).collect())
    }

    /// Whether `episode` is already evidence of `fact`.
    pub fn has_evidence(&self, fact: &str, episode: &str) -> bool {
        self.adjacent(fact, true, &["SUPPORTED_BY"])
            .iter()
            .any(|(id, _, _)| id == episode)
    }

    /// Records linked to `id` by an edge of one of `types`, read from the
    /// adjacency lists (no query): `(other id, edge type, other is a fact)`,
    /// sorted by id. `outgoing` follows edges from `id`, otherwise edges
    /// into it.
    pub fn adjacent(
        &self,
        id: &str,
        outgoing: bool,
        types: &[&str],
    ) -> Vec<(String, String, bool)> {
        let _committed = self.commit_lock.read();
        let Some(node) = self
            .db
            .find_nodes_by_property("id", &Value::from(id))
            .into_iter()
            .next()
        else {
            return Vec::new();
        };
        let lpg = self.db.store();
        let edges = if outgoing {
            lpg.edges_from(node, grafeo_core::graph::Direction::Outgoing)
                .collect::<Vec<_>>()
        } else {
            lpg.edges_to(node)
        };
        let mut out: Vec<(String, String, bool)> = edges
            .into_iter()
            .filter_map(|(other, edge)| {
                // Edge type and single properties read from the store's
                // columns: a session read builds the whole node, embedding
                // included.
                let ty = lpg.edge_type(edge)?;
                if !types.contains(&ty.as_ref()) {
                    return None;
                }
                let other_id = lpg
                    .get_node_property(other, &"id".into())
                    .as_ref()
                    .and_then(as_str)?;
                // Facts are the records with a subject.
                let is_fact = lpg.get_node_property(other, &"subject_id".into()).is_some();
                Some((other_id, ty.to_string(), is_fact))
            })
            .collect();
        out.sort();
        out
    }

    // ── Typed reads ──

    pub fn episode(&self, id: &str) -> StoreResult<Option<Episode>> {
        let rows = self.query(q::EPISODE, P::new().s("id", id))?;
        Ok(rows.first().map(|r| episode_row(r)))
    }

    pub fn all_episodes(&self) -> StoreResult<Vec<Episode>> {
        Ok(self
            .query(q::ALL_EPISODES, P::new())?
            .iter()
            .map(|r| episode_row(r))
            .collect())
    }

    pub fn all_facts(&self) -> StoreResult<Vec<Fact>> {
        Ok(self
            .query(q::ALL_FACTS, P::new())?
            .iter()
            .map(|r| fact_row(r))
            .collect())
    }

    pub fn fact(&self, id: &str) -> StoreResult<Option<Fact>> {
        let rows = self.query(q::FACT, P::new().s("id", id))?;
        Ok(rows.first().map(|r| fact_row(r)))
    }
}

fn episode_row(r: &[Value]) -> Episode {
    Episode {
        id: as_str(&r[0]).unwrap_or_default(),
        space: as_str(&r[1]).unwrap_or_default(),
        session_id: as_str(&r[2]),
        role: as_str(&r[3]).unwrap_or_default(),
        author: as_str(&r[4]),
        content: as_str(&r[5]).unwrap_or_default(),
        occurred_at: as_i64(&r[6]),
        recorded_at: as_i64(&r[7]).unwrap_or(0),
        source_ref: as_str(&r[8]),
    }
}

fn fact_row(r: &[Value]) -> Fact {
    Fact {
        id: as_str(&r[0]).unwrap_or_default(),
        space: as_str(&r[1]).unwrap_or_default(),
        subject_id: as_str(&r[2]).unwrap_or_default(),
        subject_name: as_str(&r[3]).unwrap_or_default(),
        predicate: as_str(&r[4]).unwrap_or_default(),
        value: as_str(&r[5]).unwrap_or_default(),
        value_norm: as_str(&r[6]).unwrap_or_default(),
        object_id: as_str(&r[7]),
        negated: as_bool(&r[8]),
        origin: as_str(&r[9])
            .and_then(|o| Origin::parse(&o))
            .unwrap_or(Origin::Legacy),
        status: as_str(&r[10])
            .and_then(|s| FactStatus::parse(&s))
            .unwrap_or(FactStatus::Active),
        confidence: as_f64(&r[11]),
        validity: Validity {
            from: as_i64(&r[12]),
            until: as_i64(&r[13]),
            asserted_at: as_i64(&r[14]).unwrap_or(0),
        },
        supersession: Supersession {
            until: as_i64(&r[15]),
            ended_by: as_i64(&r[16]),
        },
        knowledge: Knowledge {
            recorded_at: as_i64(&r[17]).unwrap_or(0),
            superseded_at: as_i64(&r[18]),
            retracted_at: as_i64(&r[19]),
        },
        statement: as_str(&r[20]).unwrap_or_default(),
    }
}

impl Store {
    pub fn evidence(&self, fact_id: &str) -> StoreResult<Vec<Evidence>> {
        Ok(self
            .query(q::FACT_EVIDENCE, P::new().s("fact", fact_id))?
            .iter()
            .map(|r| Evidence {
                episode_id: as_str(&r[0]).unwrap_or_default(),
                quote: as_str(&r[1]).unwrap_or_default(),
                session_id: as_str(&r[2]),
                role: as_str(&r[3]).unwrap_or_default(),
                occurred_at: as_i64(&r[4]),
            })
            .collect())
    }

    pub fn ids(&self, query: &'static str, p: P) -> StoreResult<Vec<String>> {
        Ok(self
            .query(query, p)?
            .iter()
            .filter_map(|r| r.first().and_then(as_str))
            .collect())
    }

    pub fn close(&self) -> StoreResult<()> {
        self.db.close().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs inside the next transaction, after its writes and before its
    /// commit; an error rolls the transaction back.
    type CommitHook = Box<dyn FnOnce() -> Result<(), String> + Send>;
    pub(super) static BEFORE_COMMIT: parking_lot::Mutex<Option<CommitHook>> =
        parking_lot::Mutex::new(None);

    fn mention_ops(entity: &str) -> Vec<(W, P)> {
        vec![
            (
                W::InsertEntity,
                P::new()
                    .s("id", entity)
                    .s("space", "project:x")
                    .s("type", "project")
                    .s("name", entity)
                    .s("norm", entity)
                    .list("aliases", &[]),
            ),
            (
                W::LinkEpisodeEntity,
                P::new().s("episode", "e1").s("entity", entity),
            ),
        ]
    }

    /// The reads that bypass Grafeo's snapshots wait for the commit: they
    /// see committed links, never links of a transaction still open or
    /// rolled back.
    #[test]
    fn direct_reads_never_see_an_open_transaction() {
        let s = std::sync::Arc::new(Store::in_memory().unwrap());
        episode(&s, "e1", "s1", "first");
        for (commit, entity) in [(true, "n1"), (false, "n2")] {
            let (tx, rx) = std::sync::mpsc::channel();
            let reader = s.clone();
            *BEFORE_COMMIT.lock() = Some(Box::new(move || {
                let handle = std::thread::spawn(move || reader.adjacent("e1", true, &["MENTIONS"]));
                // The reader is blocked while the transaction is open.
                std::thread::sleep(std::time::Duration::from_millis(150));
                assert!(
                    !handle.is_finished(),
                    "a read went through an open transaction"
                );
                tx.send(handle).unwrap();
                if commit {
                    Ok(())
                } else {
                    Err("aborted by the test".into())
                }
            }));
            let r = s.transaction(mention_ops(entity));
            assert_eq!(r.is_ok(), commit, "{r:?}");
            let seen = rx.recv().unwrap().join().unwrap();
            let ids: Vec<&str> = seen.iter().map(|(id, _, _)| id.as_str()).collect();
            assert_eq!(
                ids,
                vec!["n1"],
                "after {}",
                if commit { "commit" } else { "rollback" }
            );
        }
    }

    fn episode(s: &Store, id: &str, session: &str, content: &str) {
        s.exec(
            W::InsertEpisode,
            P::new()
                .s("id", id)
                .s("space", "project:x")
                .os("session_id", Some(session))
                .s("role", "user")
                .os("author", None)
                .s("content", content)
                .oi("occurred_at", None)
                .i("recorded_at", 5)
                .os("source_ref", None)
                .i("ikey", s.next_ikey()),
        )
        .unwrap();
    }

    fn fact(id: &str, ikey: i64) -> P {
        P::new()
            .s("id", id)
            .s("space", "project:x")
            .s("subject_id", "n1")
            .s("subject_name", "Bricks")
            .s("predicate", "uses_framework")
            .s("value", "Axum")
            .s("value_norm", "axum")
            .os("object_id", Some("n2"))
            .b("negated", false)
            .s("origin", "user_statement")
            .s("status", "active")
            .of("confidence", Some(0.9))
            .oi("valid_from", None)
            .oi("valid_until", None)
            .i("asserted_at", 1)
            .i("recorded_at", 2)
            .s("statement", "Bricks · uses framework: Axum")
            .i("ikey", ikey)
    }

    #[test]
    fn special_characters_travel_as_values() {
        let s = Store::in_memory().unwrap();
        let tricky = "O'Brien \\ \"q\" '}) DETACH DELETE n // 日本語 ✓\nnext line";
        episode(&s, "ep_1", "s'1", tricky);
        let e = s.episode("ep_1").unwrap().unwrap();
        assert_eq!(e.content, tricky);
        assert_eq!(e.session_id.as_deref(), Some("s'1"));
        assert_eq!(e.occurred_at, None, "unknown time stays unknown");
        assert_eq!(s.count(q::COUNT_EPISODES), 1, "nothing was injected");
    }

    #[test]
    fn every_query_and_write_runs() {
        let s = Store::in_memory().unwrap();
        episode(&s, "e1", "s1", "Bricks uses Axum");
        s.transaction(vec![
            (
                W::InsertEntity,
                P::new()
                    .s("id", "n1")
                    .s("space", "project:x")
                    .s("type", "project")
                    .s("name", "Bricks")
                    .s("norm", "bricks")
                    .list("aliases", &["cersei".into()]),
            ),
            (
                W::InsertAlias,
                P::new()
                    .s("key", &alias_key("project:x", "bricks"))
                    .s("entity_id", "n1"),
            ),
            (
                W::InsertAlias,
                P::new()
                    .s("key", &alias_key("project:x", "cersei"))
                    .s("entity_id", "n1"),
            ),
            (
                W::InsertEntity,
                P::new()
                    .s("id", "n2")
                    .s("space", "project:x")
                    .s("type", "library")
                    .s("name", "Axum")
                    .s("norm", "axum")
                    .list("aliases", &[]),
            ),
            (W::InsertFact, fact("f1", 2)),
            (W::InsertFact, fact("f2", 3)),
            (
                W::LinkEpisodeEntity,
                P::new().s("episode", "e1").s("entity", "n1"),
            ),
            (
                W::LinkFactEvidence,
                P::new()
                    .s("fact", "f1")
                    .s("episode", "e1")
                    .s("quote", "uses Axum"),
            ),
            (W::LinkFactAbout, P::new().s("fact", "f1").s("entity", "n1")),
            (
                W::LinkFactMentions,
                P::new().s("fact", "f1").s("entity", "n2"),
            ),
            (W::LinkFactAbout, P::new().s("fact", "f2").s("entity", "n1")),
            (
                W::LinkSupersedes,
                P::new().s("new", "f2").s("old", "f1").i("at", 9),
            ),
            (W::LinkContradicts, P::new().s("a", "f2").s("b", "f1")),
        ])
        .unwrap();
        for (w, p) in [
            (
                W::SetExtraction,
                P::new()
                    .s("id", "e1")
                    .s("state", "applied")
                    .i("attempts", 1)
                    .os("json", Some("{}"))
                    .os("error", None),
            ),
            (
                W::SetEmbedding,
                P::new()
                    .s("id", "e1")
                    .vector("embedding", &[1.0, 0.0])
                    .s("model", "m"),
            ),
            (
                W::SetEmbedding,
                P::new()
                    .s("id", "f1")
                    .vector("embedding", &[0.0, 1.0])
                    .s("model", "m"),
            ),
            (
                W::SupersedeFact,
                P::new()
                    .s("id", "f1")
                    .i("at", 9)
                    .oi("until", None)
                    .oi("ended_by", Some(8)),
            ),
            (
                W::SetFactStatus,
                P::new().s("id", "f2").s("status", "contested"),
            ),
            (
                W::SetEntityAliases,
                P::new().s("id", "n2").list("aliases", &["axum-rs".into()]),
            ),
            (W::InsertMeta, P::new().s("model", "m").i("dims", 2)),
            (W::SetMeta, P::new().s("model", "m2").i("dims", 3)),
        ] {
            s.exec(w, p).unwrap_or_else(|e| panic!("{w:?}: {e}"));
        }
        assert_eq!(
            s.ids(
                q::ALIAS,
                P::new().s("key", &alias_key("project:x", "cersei"))
            )
            .unwrap(),
            vec!["n1"]
        );
        assert!(
            s.ids(
                q::ALIAS,
                P::new().s("key", &alias_key("project:y", "bricks"))
            )
            .unwrap()
            .is_empty(),
            "another space has its own entities"
        );
        let ent = s.query(q::ENTITY, P::new().s("id", "n2")).unwrap();
        assert_eq!(as_strings(&ent[0][2]), vec!["axum-rs"]);
        let f1 = s.fact("f1").unwrap().unwrap();
        assert_eq!(f1.status, FactStatus::Superseded);
        assert_eq!(f1.knowledge.superseded_at, Some(9));
        assert_eq!(f1.supersession.ended_by, Some(8));
        assert_eq!(f1.validity.asserted_at, 1);
        assert_eq!(f1.confidence, Some(0.9));
        assert_eq!(s.evidence("f1").unwrap()[0].quote, "uses Axum");
        assert_eq!(
            s.facts_by_key("project:x", "n1", "uses_framework", &["contested"])
                .unwrap(),
            vec!["f2"]
        );
        assert_eq!(
            s.ids(q::SUPERSEDED_BY, P::new().s("id", "f1")).unwrap(),
            vec!["f2"]
        );
        assert_eq!(
            s.ids(q::SUPERSEDES, P::new().s("id", "f2")).unwrap(),
            vec!["f1"]
        );
        assert_eq!(
            s.ids(q::CONTRADICTIONS, P::new().s("id", "f1")).unwrap(),
            vec!["f2"]
        );
        let ents = s.adjacent("f1", true, &["ABOUT", "MENTIONS"]);
        let rels: Vec<&str> = ents.iter().map(|r| r.1.as_str()).collect();
        assert_eq!(rels.len(), 2, "{ents:?}");
        assert!(
            rels.contains(&"ABOUT") && rels.contains(&"MENTIONS"),
            "{rels:?}"
        );
        let touching = s.adjacent("n1", false, &["ABOUT", "MENTIONS"]);
        assert_eq!(touching.len(), 3, "two facts and the episode: {touching:?}");
        assert_eq!(touching.iter().filter(|r| r.2).count(), 2, "{touching:?}");
        assert!(s.adjacent("no-such-record", true, &["ABOUT"]).is_empty());
        assert_eq!(
            s.ids(q::FACTS_OF_SESSION, P::new().s("session_id", "s1"))
                .unwrap(),
            vec!["f1"]
        );
        assert_eq!(s.count(q::COUNT_FACTS), 2);
        assert!(s.has_evidence("f1", "e1"));
        assert!(!s.has_evidence("f1", "no-such-episode"));
        assert!(s
            .facts_by_key("project:y", "n1", "uses_framework", &["contested"])
            .unwrap()
            .is_empty());
        assert!(s
            .facts_by_key("project:x", "n1", "lives_in", &["contested", "superseded"])
            .unwrap()
            .is_empty());
        let items = s.query(q::INDEXED_ITEMS, P::new().s("model", "m")).unwrap();
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(as_vector(&items[0][3]).is_some());
        assert_eq!(
            s.ids(q::UNEMBEDDED, P::new().s("model", "m")).unwrap(),
            vec!["f2"]
        );
        assert_eq!(
            s.ids(
                q::PENDING_EPISODES,
                P::new().list("states", &["applied".into()])
            )
            .unwrap(),
            vec!["e1"]
        );
        assert_eq!(
            as_i64(&s.query(q::IKEY, P::new().s("id", "f2")).unwrap()[0][0]),
            Some(3)
        );
        assert_eq!(
            as_i64(&s.query(q::MAX_IKEY, P::new()).unwrap()[0][0]),
            Some(3)
        );
        assert_eq!(
            as_str(&s.query(q::META, P::new()).unwrap()[0][0]).as_deref(),
            Some("m2")
        );
        // Text indexes follow the typed writes (rebuilt after them).
        assert_eq!(s.text_search("Fact", "statement", "Axum", 5).len(), 2);
        episode(&s, "e2", "s2", "a second episode about Axum");
        assert_eq!(s.text_search("Episode", "content", "Axum", 5).len(), 2);
        s.exec(W::ClearEmbeddings, P::new()).unwrap();
        assert_eq!(
            s.ids(q::UNEMBEDDED, P::new().s("model", "m"))
                .unwrap()
                .len(),
            4
        );
        // A transaction that fails half-way leaves nothing behind.
        let r = s.transaction(vec![
            (
                W::SetFactStatus,
                P::new().s("id", "f2").s("status", "retracted"),
            ),
            (
                W::LinkFactAbout,
                P::new().s("fact", "f2").s("entity", "no-such-entity"),
            ),
        ]);
        assert!(r.is_err());
        assert_eq!(
            s.fact("f2").unwrap().unwrap().status,
            FactStatus::Contested,
            "rolled back"
        );
        assert_eq!(
            s.ids(q::EPISODES_OF_SESSION, P::new().s("session_id", "s1"))
                .unwrap(),
            vec!["e1"]
        );
        s.exec(W::DeleteNode, P::new().s("id", "e1")).unwrap();
        assert_eq!(
            as_i64(
                &s.query(q::EVIDENCE_COUNT, P::new().s("fact", "f1"))
                    .unwrap()[0][0]
            ),
            Some(0)
        );
    }
}
