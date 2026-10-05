//! Parsing of `/proc/<pid>/status`.

/// Returns the effective uid from the contents of `/proc/self/status`.
///
/// The `Uid:` line lists the real, effective, saved, and filesystem uids.
pub(crate) fn effective_uid(status: &str) -> Option<u32> {
    let line = status.lines().find_map(|line| line.strip_prefix("Uid:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::effective_uid;

    #[test]
    fn reads_the_second_uid_field() {
        let status =
            "Name:\tdemo\nUmask:\t0022\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(effective_uid(status), Some(0));
        assert_eq!(effective_uid("Uid:\t1000\t1001\t1000\t1000\n"), Some(1001));
    }

    #[test]
    fn missing_or_malformed_uid_line_is_none() {
        assert_eq!(effective_uid("Name:\tdemo\n"), None);
        assert_eq!(effective_uid("Uid:\t1000\n"), None);
        assert_eq!(effective_uid("Uid:\t1000\tabc\n"), None);
    }
}
