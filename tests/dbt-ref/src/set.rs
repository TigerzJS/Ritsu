//! A vector set: the generated vectors of several classes, with the mapping from a global index
//! back to (class, index in class).

use crate::format::{Config, Vector};
use crate::generator::{self, CLASSES, Class};
use crate::rng::fnv;

/// The vectors of one class inside a [`VectorSet`].
#[derive(Clone, Copy, Debug)]
pub struct Span {
    /// The class.
    pub class: &'static Class,
    /// Global index of the class's first vector.
    pub start: usize,
    /// Number of vectors.
    pub count: usize,
}

/// Generated vectors plus their class layout.
#[derive(Debug)]
pub struct VectorSet {
    /// Shared parameters.
    pub cfg: Config,
    /// Class spans in vector order.
    pub spans: Vec<Span>,
    /// All vectors.
    pub vectors: Vec<Vector>,
}

impl VectorSet {
    /// Generates the classes `only` (all if `None`, in table order), `cfg.units` per weight.
    ///
    /// # Errors
    ///
    /// An unknown class name.
    pub fn generate(cfg: Config, only: Option<&[String]>) -> Result<Self, String> {
        let classes: Vec<&'static Class> = match only {
            None => CLASSES.iter().collect(),
            Some(names) => names
                .iter()
                .map(|n| generator::class_by_name(n).ok_or_else(|| format!("unknown class {n:?}")))
                .collect::<Result<_, _>>()?,
        };
        let mut spans = Vec::new();
        let mut vectors = Vec::new();
        for class in classes {
            let v = generator::generate_class(&cfg, class, cfg.units);
            spans.push(Span {
                class,
                start: vectors.len(),
                count: v.len(),
            });
            vectors.extend(v);
        }
        Ok(Self {
            cfg,
            spans,
            vectors,
        })
    }

    /// The span containing global index `i`.
    #[must_use]
    pub fn span_of(&self, i: usize) -> Option<&Span> {
        self.spans
            .iter()
            .find(|s| i >= s.start && i < s.start + s.count)
    }

    /// Fingerprint of the vectors of one span (FNV-1a over their serialised records).
    #[must_use]
    pub fn span_hash(&self, span: &Span) -> u64 {
        let mut bytes = Vec::new();
        for v in &self.vectors[span.start..span.start + span.count] {
            bytes.extend(v.to_bytes());
        }
        fnv(&bytes)
    }
}
