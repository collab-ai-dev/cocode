#[cfg(target_os = "linux")]
mod linux_tests {
    use crate::seccomp::*;

    #[test]
    fn test_mode_from_str_arg() {
        assert_eq!(
            NetworkSeccompMode::from_str_arg("restricted"),
            Some(NetworkSeccompMode::Restricted)
        );
        assert_eq!(
            NetworkSeccompMode::from_str_arg("proxy-routed"),
            Some(NetworkSeccompMode::ProxyRouted)
        );
        assert_eq!(NetworkSeccompMode::from_str_arg("unknown"), None);
    }

    #[test]
    fn test_mode_roundtrip() {
        for mode in [
            NetworkSeccompMode::Restricted,
            NetworkSeccompMode::ProxyRouted,
        ] {
            let as_str = mode.as_str_arg();
            assert_eq!(NetworkSeccompMode::from_str_arg(as_str), Some(mode));
        }
    }

    #[test]
    fn test_determine_mode_full_network_no_proxy() {
        assert_eq!(
            determine_seccomp_mode(/*allow_network=*/ true, /*proxy=*/ false),
            None
        );
    }

    #[test]
    fn test_determine_mode_network_blocked() {
        assert_eq!(
            determine_seccomp_mode(/*allow_network=*/ false, /*proxy=*/ false),
            Some(NetworkSeccompMode::Restricted)
        );
    }

    #[test]
    fn test_determine_mode_proxy_active() {
        assert_eq!(
            determine_seccomp_mode(/*allow_network=*/ true, /*proxy=*/ true),
            Some(NetworkSeccompMode::ProxyRouted)
        );
        assert_eq!(
            determine_seccomp_mode(/*allow_network=*/ false, /*proxy=*/ true),
            Some(NetworkSeccompMode::ProxyRouted)
        );
    }

    #[test]
    fn test_compile_restricted_filter() {
        let bpf = compile_seccomp_filter(NetworkSeccompMode::Restricted)
            .expect("compile restricted filter");
        // Filter must have instructions: arch check + syscall rules + default return
        assert!(
            bpf.len() > 10,
            "BPF should have >10 instructions, got {}",
            bpf.len()
        );
    }

    #[test]
    fn test_compile_proxy_routed_filter() {
        let bpf = compile_seccomp_filter(NetworkSeccompMode::ProxyRouted)
            .expect("compile proxy-routed filter");
        assert!(
            bpf.len() > 10,
            "BPF should have >10 instructions, got {}",
            bpf.len()
        );
    }

    #[test]
    fn test_both_modes_compile_different_filters() {
        let restricted =
            compile_seccomp_filter(NetworkSeccompMode::Restricted).expect("compile restricted");
        let proxy_routed =
            compile_seccomp_filter(NetworkSeccompMode::ProxyRouted).expect("compile proxy-routed");
        // Modes produce different filter programs
        assert_ne!(restricted.len(), proxy_routed.len());
    }
}
