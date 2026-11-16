//! `Range` / `If-Range` parsing shared by every transfer path (#211).

use axum::http::{header, HeaderMap};

use crate::storage::RequestRange;

/// Whether a client `Range` may be honoured under `If-Range` (#211).
///
/// Only a strong entity-tag equal to the current one matches. A missing
/// `If-Range` keeps the plain `Range` behaviour, and an HTTP-date is treated as
/// a mismatch: the archive is restarted from zero rather than spliced, which is
/// always a valid response and never serves inconsistent bytes.
pub(crate) fn if_range_matches(headers: &HeaderMap, etag: Option<&str>) -> bool {
    let Some(value) = headers.get(header::IF_RANGE) else {
        return true;
    };
    let Some(etag) = etag else {
        return false;
    };
    value.to_str().is_ok_and(|value| value.trim() == etag)
}

/// Last byte of a full (`200`) response, when its length is known.
pub(crate) fn full_range_end(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|length| length.checked_sub(1))
}

/// Bounds of `Content-Range: bytes <start>-<end>/<total>`.
pub(crate) fn content_range_bounds(value: &str) -> Option<(u64, u64)> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, rest) = range.split_once('-')?;
    let (end, _) = rest.split_once('/')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
}

/// Parses `bytes=start-end`, `bytes=start-` and `bytes=-length`.
pub(crate) fn parse_range(value: &str) -> Option<Option<RequestRange>> {
    let value = value.trim();
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() {
        let length = end.parse::<u64>().ok()?;
        return (length > 0).then_some(Some(RequestRange::Suffix(length)));
    }
    let start = start.parse::<u64>().ok()?;
    if end.is_empty() {
        return Some(Some(RequestRange::Offset(start)));
    }
    let end = end.parse::<u64>().ok()?;
    (start <= end).then_some(Some(RequestRange::Bounded { start, end }))
}

#[cfg(test)]
mod tests {
    use super::parse_range;
    use crate::storage::RequestRange;

    #[test]
    fn range_parser_handles_the_three_http_forms() {
        assert_eq!(
            parse_range("bytes=0-99").flatten(),
            Some(RequestRange::Bounded { start: 0, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=10-").flatten(),
            Some(RequestRange::Offset(10))
        );
        assert_eq!(
            parse_range("bytes=-10").flatten(),
            Some(RequestRange::Suffix(10))
        );
        assert!(parse_range("bytes=0-1,4-5").is_none());
        assert!(parse_range("bytes=9-3").is_none());
        assert!(parse_range("items=0-1").is_none());
    }
}
