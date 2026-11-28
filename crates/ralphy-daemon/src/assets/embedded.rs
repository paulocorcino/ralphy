//! The embedded workbench files, read across both trees: [`crate::UI`] holds a
//! file as it is, and [`crate::UI_GZ`] holds a vendored file as its gzip body
//! (`build/ui_gzip.rs`). A path is in exactly one of them.

use std::borrow::Cow;
use std::io::Read;

/// The bytes the binary holds for one embedded file.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stored {
    /// The file as it is.
    Raw(&'static [u8]),
    /// The gzip body of the file.
    Gzip(&'static [u8]),
}

impl Stored {
    /// The bytes as stored: the file, or its gzip body.
    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Self::Raw(bytes) | Self::Gzip(bytes) => bytes,
        }
    }

    /// The file as it is, inflated when it is stored gzipped.
    pub(crate) fn contents(self) -> std::io::Result<Cow<'static, [u8]>> {
        match self {
            Self::Raw(bytes) => Ok(Cow::Borrowed(bytes)),
            Self::Gzip(body) => {
                let mut raw = Vec::with_capacity(body.len().saturating_mul(3));
                flate2::read::GzDecoder::new(body).read_to_end(&mut raw)?;
                Ok(Cow::Owned(raw))
            }
        }
    }
}

/// The embedded file at `path` (slash-separated, relative to `assets/ui`).
pub(crate) fn file(path: &str) -> Option<Stored> {
    if let Some(f) = crate::UI.get_file(path) {
        return Some(Stored::Raw(f.contents()));
    }
    crate::UI_GZ
        .get_file(path)
        .map(|f| Stored::Gzip(f.contents()))
}

/// Every embedded file with its slash-separated path, in no set order.
pub(crate) fn every_file() -> Vec<(String, Stored)> {
    let mut out = Vec::new();
    walk(&crate::UI, Stored::Raw, &mut out);
    walk(&crate::UI_GZ, Stored::Gzip, &mut out);
    out
}

fn walk(
    dir: &'static include_dir::Dir<'static>,
    stored: fn(&'static [u8]) -> Stored,
    out: &mut Vec<(String, Stored)>,
) {
    for f in dir.files() {
        let path = f.path().to_string_lossy().replace('\\', "/");
        out.push((path, stored(f.contents())));
    }
    for sub in dir.dirs() {
        walk(sub, stored, out);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
    use http_body_util::BodyExt;

    use super::{file, Stored};

    const MERMAID: &str = "vendor/mermaid.min.js";

    fn source() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
        std::fs::read(path.join(MERMAID)).expect("the vendored source is readable")
    }

    async fn served(accept_encoding: Option<&str>) -> (Option<String>, Vec<u8>) {
        let mut headers = HeaderMap::new();
        if let Some(value) = accept_encoding {
            headers.insert(
                header::ACCEPT_ENCODING,
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let uri: Uri = format!("/{MERMAID}").parse().unwrap();
        let resp = crate::ui_asset(headers, uri).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let encoding = resp
            .headers()
            .get(header::CONTENT_ENCODING)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (encoding, body)
    }

    // pin: the binary holds a big vendored script as its gzip body, not as it is.
    #[test]
    fn a_big_vendored_script_is_stored_gzipped() {
        let stored = file(MERMAID).expect("mermaid is embedded");
        assert!(
            matches!(stored, Stored::Gzip(_)),
            "{MERMAID} is stored gzipped"
        );
        assert!(
            stored.bytes().len() < source().len() / 2,
            "gzip halves it at least"
        );
    }

    #[tokio::test]
    async fn a_gzipped_file_is_served_as_its_source_with_and_without_gzip() {
        let (encoding, body) = served(Some("gzip, deflate, br")).await;
        assert_eq!(encoding.as_deref(), Some("gzip"));
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(&body[..])
            .read_to_end(&mut inflated)
            .unwrap();
        assert!(inflated == source(), "the gzip body inflates to the source");

        let (encoding, body) = served(None).await;
        assert_eq!(encoding, None, "a client without gzip gets no encoding");
        assert!(
            body == source(),
            "a client without gzip gets the source bytes"
        );
    }
}
