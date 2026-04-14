//! Input-validation helpers used across nilbox-core.
//!
//! All functions are pure (no I/O) so they are easy to unit-test.

/// Returns `true` when `path` looks like a Linux virtio block device.
///
/// Accepted patterns: `/dev/vd[a-z]` (whole disk) or `/dev/vd[a-z][0-9]+` (partition).
pub fn is_valid_device_path(path: &str) -> bool {
    // ^/dev/vd[a-z][0-9]*$
    let Some(rest) = path.strip_prefix("/dev/vd") else { return false };
    let mut chars = rest.chars();
    let Some(disk) = chars.next() else { return false };
    if !disk.is_ascii_lowercase() {
        return false;
    }
    let tail: String = chars.collect();
    tail.is_empty() || tail.chars().all(|c| c.is_ascii_digit())
}

/// Returns `true` when `s` contains only `[a-zA-Z0-9_-]`.
///
/// Suitable for provider names, filenames (without extension), etc.
pub fn is_valid_safe_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Returns `true` when `s` contains only `[a-zA-Z0-9._-]`.
///
/// Suitable for application IDs, package identifiers, etc.
pub fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Returns `true` when `url` has an `http` or `https` scheme.
///
/// Rejects `javascript:`, `data:`, `file:`, etc.
pub fn is_valid_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_valid() {
        assert!(is_valid_device_path("/dev/vda"));
        assert!(is_valid_device_path("/dev/vda1"));
        assert!(is_valid_device_path("/dev/vdb12"));
    }

    #[test]
    fn device_path_invalid() {
        assert!(!is_valid_device_path("/dev/sda1"));
        assert!(!is_valid_device_path("/dev/vda1; rm -rf /"));
        assert!(!is_valid_device_path("../../etc/passwd"));
        assert!(!is_valid_device_path("/dev/vd"));
        assert!(!is_valid_device_path("/dev/vdA1"));
        assert!(!is_valid_device_path("/dev/vda1 "));
    }

    #[test]
    fn safe_name_valid() {
        assert!(is_valid_safe_name("github"));
        assert!(is_valid_safe_name("my-provider_1"));
    }

    #[test]
    fn safe_name_invalid() {
        assert!(!is_valid_safe_name(""));
        assert!(!is_valid_safe_name("../../etc"));
        assert!(!is_valid_safe_name("name with spaces"));
        assert!(!is_valid_safe_name("foo/bar"));
    }

    #[test]
    fn identifier_valid() {
        assert!(is_valid_identifier("com.example.app-1"));
        assert!(is_valid_identifier("my_app.v2"));
    }

    #[test]
    fn identifier_invalid() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("app id"));
        assert!(!is_valid_identifier("../hack"));
        assert!(!is_valid_identifier("foo;bar"));
    }

    #[test]
    fn http_url_valid() {
        assert!(is_valid_http_url("http://localhost:8080/admin"));
        assert!(is_valid_http_url("https://example.com/path"));
    }

    #[test]
    fn http_url_invalid() {
        assert!(!is_valid_http_url("javascript:alert(1)"));
        assert!(!is_valid_http_url("data:text/html,<h1>XSS</h1>"));
        assert!(!is_valid_http_url("file:///etc/passwd"));
        assert!(!is_valid_http_url("ftp://example.com"));
    }
}
