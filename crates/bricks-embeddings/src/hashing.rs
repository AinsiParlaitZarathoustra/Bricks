//! A deterministic, local embedder: feature hashing of words and word pairs.
//!
//! It needs no network and no key, and gives the same vector for the same
//! text on every machine — for tests, offline use and reproducible
//! benchmarks. It is lexical, not semantic: paraphrases without shared words
//! are not close. Do not mistake its results for those of a trained model.

use crate::{EmbeddingError, EmbeddingProvider};
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct HashingEmbeddings {
    dims: usize,
}

impl HashingEmbeddings {
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(8) }
    }

    pub fn embed_sync(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dims];
        let words: Vec<String> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(fold)
            .collect();
        let mut add = |token: &str, weight: f32| {
            let h = fnv1a(token.as_bytes());
            let i = (h % self.dims as u64) as usize;
            let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
            v[i] += sign * weight;
        };
        for w in &words {
            add(w, 1.0);
        }
        for pair in words.windows(2) {
            add(&format!("{} {}", pair[0], pair[1]), 0.5);
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

fn fold(w: &str) -> String {
    w.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            c => c,
        })
        .collect()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[async_trait]
impl EmbeddingProvider for HashingEmbeddings {
    fn name(&self) -> &str {
        "hashing"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> String {
        format!("hashing-v1:{}", self.dims)
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(texts.iter().map(|t| self.embed_sync(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_lexical() {
        let e = HashingEmbeddings::new(64);
        let a = e.embed_sync("Bricks uses Actix");
        assert_eq!(a, e.embed_sync("bricks USES actix"));
        let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>();
        assert!(
            dot(&a, &e.embed_sync("Actix in Bricks")) > dot(&a, &e.embed_sync("tea and biscuits"))
        );
        assert!((dot(&a, &a) - 1.0).abs() < 1e-5);
    }
}
