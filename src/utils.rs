//! Internal utilities for the multi-tier cache system.

/// Redact credentials from a URL for safe logging.
///
/// Replaces any password component with `***` so that connection URLs
/// can be logged without leaking secrets.
///
/// # Examples
///
/// ```
/// # use multi_tier_cache::utils::redact_url;
/// assert_eq!(redact_url("redis://user:s3cret@host:6379/0"), "redis://user:***@host:6379/0");
/// assert_eq!(redact_url("redis://host:6379"), "redis://host:6379");
/// assert_eq!(redact_url("memcache://127.0.0.1:11211"), "memcache://127.0.0.1:11211");
/// assert_eq!(redact_url("not-a-url"), "not-a-url");
/// ```
#[must_use]
pub fn redact_url(url: &str) -> String {
    // Find the scheme separator "://"
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = scheme_end + 3; // index right after "://"

    // Find the host portion boundary (the first '/' after the authority, if any)
    let authority_end = url[after_scheme..]
        .find('/')
        .map_or(url.len(), |pos| after_scheme + pos);

    let authority = &url[after_scheme..authority_end];

    // Check for userinfo section (contains '@')
    let Some(at_pos) = authority.find('@') else {
        // No credentials in the URL
        return url.to_string();
    };

    let userinfo = &authority[..at_pos];

    // Check for password (contains ':')
    let Some(colon_pos) = userinfo.find(':') else {
        // Username only, no password to redact
        return url.to_string();
    };

    // Rebuild the URL with the password replaced by "***"
    let username = &userinfo[..colon_pos];
    let host_and_rest = &url[after_scheme + at_pos..]; // includes '@' onwards

    format!(
        "{}://{}:***{}{}",
        &url[..scheme_end],
        username,
        host_and_rest,
        ""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password_in_redis_url() {
        assert_eq!(
            redact_url("redis://user:s3cret@localhost:6379/0"),
            "redis://user:***@localhost:6379/0"
        );
    }

    #[test]
    fn redacts_password_in_memcache_url() {
        assert_eq!(
            redact_url("memcache://admin:password123@10.0.0.1:11211"),
            "memcache://admin:***@10.0.0.1:11211"
        );
    }

    #[test]
    fn preserves_url_without_credentials() {
        assert_eq!(
            redact_url("redis://127.0.0.1:6379"),
            "redis://127.0.0.1:6379"
        );
    }

    #[test]
    fn preserves_url_with_username_only() {
        assert_eq!(
            redact_url("redis://user@localhost:6379"),
            "redis://user@localhost:6379"
        );
    }

    #[test]
    fn handles_url_with_path() {
        assert_eq!(
            redact_url("redis://user:pass@host:6379/0"),
            "redis://user:***@host:6379/0"
        );
    }

    #[test]
    fn handles_non_url_string() {
        assert_eq!(redact_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn handles_empty_password() {
        assert_eq!(
            redact_url("redis://user:@localhost:6379"),
            "redis://user:***@localhost:6379"
        );
    }

    #[test]
    fn handles_special_chars_in_password() {
        assert_eq!(
            redact_url("redis://user:p%40ss!w0rd@host:6379"),
            "redis://user:***@host:6379"
        );
    }
}
