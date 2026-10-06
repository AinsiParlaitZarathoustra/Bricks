//! The vector projection: an in-process HNSW index (USearch) over the
//! embeddings stored in Grafeo, with a catalogue of what each key is.
//!
//! The store is authoritative: vectors live on their `:Episode` / `:Fact`
//! nodes with the id of the model that produced them. The index is rebuilt
//! from the store when the memory opens and updated as records are
//! embedded or change; it is never written to its own file, so there is no
//! second store to keep consistent and nothing to repair after a crash —
//! records without a vector of the current model are simply embedded again.
//!
//! The catalogue (space, status, validity, knowledge times) lets the
//! search apply scope and time constraints *during* the graph search
//! (USearch filtered search), so a restrictive filter does not empty the
//! result after the fact.
//!
//! USearch is a C++ library (built by `cc`/`cxx` with C++17): a C++
//! toolchain is required to build this crate. HNSW search is approximate.

use super::model::{Episode, Fact};
use cersei_embeddings::{Metric, VectorIndex};
use std::collections::HashMap;

/// What an index key stands for.
#[derive(Debug, Clone)]
pub enum Item {
    Fact(Box<Fact>),
    Episode(Box<Episode>),
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::Fact(f) => &f.id,
            Item::Episode(e) => &e.id,
        }
    }

    pub fn space(&self) -> &str {
        match self {
            Item::Fact(f) => &f.space,
            Item::Episode(e) => &e.space,
        }
    }
}

pub struct Projection {
    index: VectorIndex,
    pub model: String,
    pub dims: usize,
    items: HashMap<u64, Item>,
    by_id: HashMap<String, u64>,
}

impl Projection {
    pub fn new(model: &str, dims: usize) -> Result<Self, String> {
        let index = VectorIndex::new(dims, Metric::Cosine).map_err(|e| e.to_string())?;
        Ok(Self {
            index,
            model: model.to_string(),
            dims,
            items: HashMap::new(),
            by_id: HashMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add or replace a record's vector.
    pub fn upsert(&mut self, key: u64, item: Item, vector: &[f32]) -> Result<(), String> {
        if vector.len() != self.dims {
            return Err(format!(
                "vector of {} dimensions for an index of {} ({})",
                vector.len(),
                self.dims,
                self.model
            ));
        }
        if self.index.contains(key) {
            let _ = self.index.remove(key);
        }
        if self.index.capacity() <= self.index.len() + 1 {
            let grow = (self.index.capacity() * 2).max(64);
            self.index.reserve(grow).map_err(|e| e.to_string())?;
        }
        self.index.add(key, vector).map_err(|e| e.to_string())?;
        self.by_id.insert(item.id().to_string(), key);
        self.items.insert(key, item);
        Ok(())
    }

    /// Update what is known of a record (status, validity…) without
    /// touching its vector.
    pub fn refresh(&mut self, item: Item) {
        if let Some(key) = self.by_id.get(item.id()).copied() {
            self.items.insert(key, item);
        }
    }

    pub fn remove_id(&mut self, id: &str) {
        if let Some(key) = self.by_id.remove(id) {
            let _ = self.index.remove(key);
            self.items.remove(&key);
        }
    }

    pub fn item(&self, key: u64) -> Option<&Item> {
        self.items.get(&key)
    }

    pub fn key_of(&self, id: &str) -> Option<u64> {
        self.by_id.get(id).copied()
    }

    /// The catalogued fact with this id (embedded facts only).
    pub fn fact(&self, id: &str) -> Option<&Fact> {
        match self.items.get(self.by_id.get(id)?)? {
            Item::Fact(f) => Some(f),
            Item::Episode(_) => None,
        }
    }

    /// The `k` nearest records that `admit` accepts, best first, with
    /// cosine similarity. Ties are broken by record id.
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        admit: impl Fn(&Item) -> bool,
    ) -> Result<Vec<(u64, f32)>, String> {
        if self.items.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        if query.len() != self.dims {
            return Err(format!(
                "query vector of {} dimensions for an index of {}",
                query.len(),
                self.dims
            ));
        }
        let hits = self
            .index
            .search_filtered(query, k, |key| self.items.get(&key).is_some_and(&admit))
            .map_err(|e| e.to_string())?;
        let mut out: Vec<(u64, f32)> = hits
            .into_iter()
            .filter(|h| self.items.contains_key(&h.key))
            .map(|h| (h.key, h.similarity))
            .collect();
        out.sort_by(|a, b| {
            b.1.total_cmp(&a.1).then_with(|| {
                let ia = self.items.get(&a.0).map(|i| i.id()).unwrap_or("");
                let ib = self.items.get(&b.0).map(|i| i.id()).unwrap_or("");
                ia.cmp(ib)
            })
        });
        Ok(out)
    }
}
