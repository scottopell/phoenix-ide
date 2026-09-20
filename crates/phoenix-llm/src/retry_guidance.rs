use reqwest::header::HeaderMap;
use std::time::{Duration, SystemTime};

const MAX_AUTOMATIC_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAfter {
    WithinLimit(Duration),
    ExceedsLimit(Duration),
}

impl RetryAfter {
    #[must_use]
    pub fn duration(self) -> Duration {
        match self {
            Self::WithinLimit(duration) | Self::ExceedsLimit(duration) => duration,
        }
    }

    fn classify(duration: Duration) -> Self {
        if duration <= MAX_AUTOMATIC_RETRY_AFTER {
            Self::WithinLimit(duration)
        } else {
            Self::ExceedsLimit(duration)
        }
    }
}

pub(crate) fn retry_after_from_headers(headers: &HeaderMap) -> Option<RetryAfter> {
    retry_after_from_headers_at(headers, SystemTime::now())
}

fn retry_after_from_headers_at(headers: &HeaderMap, now: SystemTime) -> Option<RetryAfter> {
    if let Some(value) = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(seconds) = value.trim().parse::<u64>() {
            return Some(RetryAfter::classify(Duration::from_secs(seconds)));
        }
        if let Ok(deadline) = httpdate::parse_http_date(value.trim()) {
            return deadline.duration_since(now).ok().map(RetryAfter::classify);
        }
    }

    ["retry-after-ms", "x-retry-after-ms"]
        .into_iter()
        .find_map(|name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_millis)
                .map(RetryAfter::classify)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn parses_delta_seconds() {
        assert_eq!(
            retry_after_from_headers_at(&headers("retry-after", "17"), SystemTime::UNIX_EPOCH),
            Some(RetryAfter::WithinLimit(Duration::from_secs(17)))
        );
    }

    #[test]
    fn parses_http_date_against_injected_clock() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        let deadline = now + Duration::from_secs(23);
        let value = httpdate::fmt_http_date(deadline);
        assert_eq!(
            retry_after_from_headers_at(&headers("retry-after", &value), now),
            Some(RetryAfter::WithinLimit(Duration::from_secs(23)))
        );
    }

    #[test]
    fn rejects_invalid_and_past_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        assert_eq!(
            retry_after_from_headers_at(&headers("retry-after", "later"), now),
            None
        );
        assert_eq!(
            retry_after_from_headers_at(
                &headers(
                    "retry-after",
                    &httpdate::fmt_http_date(now - Duration::from_secs(1))
                ),
                now
            ),
            None
        );
    }

    #[test]
    fn parses_provider_millisecond_equivalents() {
        for name in ["retry-after-ms", "x-retry-after-ms"] {
            assert_eq!(
                retry_after_from_headers_at(&headers(name, "1250"), SystemTime::UNIX_EPOCH),
                Some(RetryAfter::WithinLimit(Duration::from_millis(1250)))
            );
        }
    }

    #[test]
    fn preserves_values_above_policy_limit() {
        let guidance =
            retry_after_from_headers_at(&headers("retry-after", "31"), SystemTime::UNIX_EPOCH);
        assert_eq!(
            guidance,
            Some(RetryAfter::ExceedsLimit(Duration::from_secs(31)))
        );
        assert_eq!(
            guidance.map(RetryAfter::duration),
            Some(Duration::from_secs(31))
        );
    }
}
