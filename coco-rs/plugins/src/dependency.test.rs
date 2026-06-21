use super::*;

#[test]
fn qualify_inherits_marketplace() {
    let declaring = PluginId::parse("a@market");
    let dep = qualify_dependency("b", &declaring);
    assert_eq!(dep, PluginId::parse("b@market"));
}

#[test]
fn qualify_keeps_existing_marketplace() {
    let declaring = PluginId::parse("a@market");
    let dep = qualify_dependency("b@other", &declaring);
    assert_eq!(dep, PluginId::parse("b@other"));
}

#[test]
fn qualify_inline_keeps_bare() {
    let declaring = PluginId::parse("a@inline");
    let dep = qualify_dependency("b", &declaring);
    assert_eq!(dep, PluginId::parse("b"));
    assert!(dep.marketplace.is_none());
}

#[tokio::test]
async fn resolve_simple_chain() {
    let root = PluginId::parse("a@m");
    let lookup = |id: PluginId| async move {
        match id.name.as_str() {
            "a" => Some(DependencyLookupResult {
                dependencies: vec!["b".into()],
            }),
            "b" => Some(DependencyLookupResult {
                dependencies: vec!["c".into()],
            }),
            "c" => Some(DependencyLookupResult::default()),
            _ => None,
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &HashSet::new(), &HashSet::new()).await;
    match r {
        ResolutionResult::Ok { closure } => {
            assert_eq!(closure.len(), 3);
            assert_eq!(closure[0], PluginId::parse("c@m"));
            assert_eq!(closure[1], PluginId::parse("b@m"));
            assert_eq!(closure[2], PluginId::parse("a@m"));
        }
        other => panic!("expected ok, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_detects_cycle() {
    let root = PluginId::parse("a@m");
    let lookup = |id: PluginId| async move {
        match id.name.as_str() {
            "a" => Some(DependencyLookupResult {
                dependencies: vec!["b".into()],
            }),
            "b" => Some(DependencyLookupResult {
                dependencies: vec!["a".into()],
            }),
            _ => None,
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &HashSet::new(), &HashSet::new()).await;
    assert!(matches!(r, ResolutionResult::Cycle { .. }));
}

#[tokio::test]
async fn resolve_skips_already_enabled_dep() {
    let root = PluginId::parse("a@m");
    let already: HashSet<PluginId> = std::iter::once(PluginId::parse("b@m")).collect();
    let lookup = |id: PluginId| async move {
        match id.name.as_str() {
            "a" => Some(DependencyLookupResult {
                dependencies: vec!["b".into()],
            }),
            _ => None, // b is in already_enabled, must be skipped before lookup
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &already, &HashSet::new()).await;
    match r {
        ResolutionResult::Ok { closure } => {
            assert_eq!(closure.len(), 1);
            assert_eq!(closure[0], root);
        }
        other => panic!("expected ok, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_self_dep_detected_as_cycle() {
    // Edge case: plugin a@m declares a@m as its own dep. This is a cycle
    // (id appears twice in the stack on the recursive walk); the
    // root-never-skipped rule applies on entry, so we don't short-circuit
    // out of the already-enabled check before the cycle detector fires.
    let root = PluginId::parse("a@m");
    let already: HashSet<PluginId> = std::iter::once(PluginId::parse("a@m")).collect();
    let lookup = |id: PluginId| async move {
        if id.name == "a" {
            Some(DependencyLookupResult {
                dependencies: vec!["a".into()],
            })
        } else {
            None
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &already, &HashSet::new()).await;
    match r {
        ResolutionResult::Cycle { chain } => {
            // Chain shows root → root.
            assert_eq!(chain.len(), 2);
            assert_eq!(chain[0], root);
            assert_eq!(chain[1], root);
        }
        other => panic!("expected Cycle for self-dep, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_root_never_skipped_even_if_already_enabled() {
    let root = PluginId::parse("a@m");
    let already: HashSet<PluginId> = std::iter::once(root.clone()).collect();
    let lookup = |id: PluginId| async move {
        match id.name.as_str() {
            "a" => Some(DependencyLookupResult::default()),
            _ => None,
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &already, &HashSet::new()).await;
    match r {
        ResolutionResult::Ok { closure } => {
            assert_eq!(closure, vec![root]);
        }
        other => panic!("expected ok, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_blocks_cross_marketplace_by_default() {
    let root = PluginId::parse("a@m");
    let lookup = |id: PluginId| async move {
        if id.name == "a" {
            Some(DependencyLookupResult {
                dependencies: vec!["b@other".into()],
            })
        } else {
            Some(DependencyLookupResult::default())
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &HashSet::new(), &HashSet::new()).await;
    assert!(matches!(r, ResolutionResult::CrossMarketplace { .. }));
}

#[tokio::test]
async fn resolve_allows_cross_marketplace_when_listed() {
    let root = PluginId::parse("a@m");
    let allow: HashSet<String> = std::iter::once("other".to_string()).collect();
    let lookup = |id: PluginId| async move {
        match id.name.as_str() {
            "a" => Some(DependencyLookupResult {
                dependencies: vec!["b@other".into()],
            }),
            "b" => Some(DependencyLookupResult::default()),
            _ => None,
        }
    };
    let r = resolve_dependency_closure(&root, lookup, &HashSet::new(), &allow).await;
    assert!(matches!(r, ResolutionResult::Ok { .. }));
}

#[test]
fn verify_and_demote_simple() {
    let plugins = vec![
        DemotePluginRecord {
            source: PluginId::parse("a@m"),
            enabled: true,
            dependencies: vec!["b".into()],
        },
        DemotePluginRecord {
            source: PluginId::parse("b@m"),
            enabled: false,
            dependencies: vec![],
        },
    ];
    let r = verify_and_demote(&plugins);
    assert_eq!(r.demoted.len(), 1);
    assert!(r.demoted.contains(&PluginId::parse("a@m")));
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].reason, DemotionReason::NotEnabled);
}

#[test]
fn verify_and_demote_fixed_point() {
    // a→b, b→c. c disabled. Both a and b must demote.
    let plugins = vec![
        DemotePluginRecord {
            source: PluginId::parse("a@m"),
            enabled: true,
            dependencies: vec!["b".into()],
        },
        DemotePluginRecord {
            source: PluginId::parse("b@m"),
            enabled: true,
            dependencies: vec!["c".into()],
        },
        DemotePluginRecord {
            source: PluginId::parse("c@m"),
            enabled: false,
            dependencies: vec![],
        },
    ];
    let r = verify_and_demote(&plugins);
    assert_eq!(r.demoted.len(), 2);
    assert!(r.demoted.contains(&PluginId::parse("a@m")));
    assert!(r.demoted.contains(&PluginId::parse("b@m")));
}

#[test]
fn verify_and_demote_not_found_vs_not_enabled() {
    let plugins = vec![DemotePluginRecord {
        source: PluginId::parse("a@m"),
        enabled: true,
        dependencies: vec!["ghost@m".into()],
    }];
    let r = verify_and_demote(&plugins);
    assert_eq!(r.errors[0].reason, DemotionReason::NotFound);
}

#[test]
fn reverse_dependents_finds_callers() {
    let plugins = vec![
        DemotePluginRecord {
            source: PluginId::parse("a@m"),
            enabled: true,
            dependencies: vec!["b".into()],
        },
        DemotePluginRecord {
            source: PluginId::parse("b@m"),
            enabled: true,
            dependencies: vec![],
        },
        DemotePluginRecord {
            source: PluginId::parse("c@m"),
            enabled: true,
            dependencies: vec!["b".into()],
        },
    ];
    let r = find_reverse_dependents(&PluginId::parse("b@m"), &plugins);
    assert_eq!(r.len(), 2);
    assert!(r.contains(&PluginId::parse("a@m")));
    assert!(r.contains(&PluginId::parse("c@m")));
}

#[test]
fn format_suffixes() {
    let none: Vec<PluginId> = vec![];
    let one = vec![PluginId::parse("a@m")];
    let two = vec![PluginId::parse("a@m"), PluginId::parse("b@m")];

    assert_eq!(format_dependency_count_suffix(&none), "");
    assert_eq!(format_dependency_count_suffix(&one), " (+ 1 dependency)");
    assert_eq!(format_dependency_count_suffix(&two), " (+ 2 dependencies)");

    assert_eq!(format_reverse_dependents_suffix(&none), "");
    assert_eq!(
        format_reverse_dependents_suffix(&two),
        " — warning: required by a, b"
    );
}
