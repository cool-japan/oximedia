//! Importers for various timeline formats (EDL, XML, AAF, FCPXML).

pub mod aaf;
pub mod edl;
pub mod fcpxml;
pub(crate) mod xmeml;
pub mod xml;

use crate::error::ConformResult;
use crate::types::ClipReference;
use std::path::Path;

/// Trait for timeline importers.
pub trait TimelineImporter {
    /// Import clips from a timeline file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be parsed.
    fn import<P: AsRef<Path>>(&self, path: P) -> ConformResult<Vec<ClipReference>>;
}
