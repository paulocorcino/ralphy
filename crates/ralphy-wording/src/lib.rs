//! The format of the wording catalog; the text of each domain lives in `assets/wording/`.

mod domain_file;
mod fill;
mod text_domain;
mod value_set;

pub use domain_file::{DomainFile, ValueWording};
pub use fill::{fill, placeholders, FillError};
pub use text_domain::TextDomain;
pub use value_set::{check_values, ValueSet};
