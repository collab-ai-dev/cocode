use super::*;

#[cfg(unix)]
mod unix_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::ffi::OsStringExt;

    /// Helper for testing prefix matching without removing real env vars.
    fn collect_keys_with_prefix(
        vars: impl IntoIterator<Item = (OsString, OsString)>,
        prefix: &[u8],
    ) -> Vec<OsString> {
        vars.into_iter()
            .filter_map(|(key, _)| {
                key.as_os_str()
                    .as_bytes()
                    .starts_with(prefix)
                    .then_some(key)
            })
            .collect()
    }

    #[test]
    fn env_keys_with_prefix_handles_non_utf8_entries() {
        let non_utf8_key1 = OsStr::from_bytes(b"R\xD6DBURK").to_os_string();
        assert!(non_utf8_key1.clone().into_string().is_err());
        let non_utf8_key2 = OsString::from_vec(vec![b'L', b'D', b'_', 0xF0]);
        assert!(non_utf8_key2.clone().into_string().is_err());

        let non_utf8_value = OsString::from_vec(vec![0xF0, 0x9F, 0x92, 0xA9]);

        let keys = collect_keys_with_prefix(
            vec![
                (non_utf8_key1, non_utf8_value.clone()),
                (non_utf8_key2.clone(), non_utf8_value),
            ],
            b"LD_",
        );
        assert_eq!(
            keys,
            vec![non_utf8_key2],
            "non-UTF-8 env entries with LD_ prefix should be retained"
        );
    }

    #[test]
    fn env_keys_with_prefix_filters_only_matching_keys() {
        let ld_test_var = OsStr::from_bytes(b"LD_TEST");
        let vars = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (ld_test_var.to_os_string(), OsString::from("1")),
            (OsString::from("DYLD_FOO"), OsString::from("bar")),
        ];

        let keys = collect_keys_with_prefix(vars, b"LD_");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].as_os_str(), ld_test_var);
    }

    #[test]
    fn env_keys_with_prefix_empty_input() {
        let keys = collect_keys_with_prefix(vec![], b"LD_");
        assert!(keys.is_empty());
    }

    #[test]
    fn env_keys_with_prefix_no_matches() {
        let vars = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("HOME"), OsString::from("/root")),
        ];
        let keys = collect_keys_with_prefix(vars, b"LD_");
        assert!(keys.is_empty());
    }
}
