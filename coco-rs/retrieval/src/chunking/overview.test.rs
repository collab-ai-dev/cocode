use super::*;

fn make_tag(kind: TagKind, name: &str, start: i32, end: i32) -> CodeTag {
    CodeTag {
        kind,
        name: name.to_string(),
        start_line: start,
        end_line: end,
        start_byte: 0,
        end_byte: 0,
        signature: None,
        docs: None,
        is_definition: true,
    }
}

#[test]
fn test_generate_overview_for_class() {
    let source = r#"class UserService {
    fn get_user(id: i64) -> User {
        // implementation
        self.db.find(id)
    }

    fn create_user(name: &str) -> User {
        // implementation
        User::new(name)
    }

    fn delete_user(id: i64) {
        // implementation
        self.db.delete(id)
    }
}"#;

    let tags = vec![
        make_tag(TagKind::Class, "UserService", 0, 14),
        make_tag(TagKind::Method, "get_user", 1, 4),
        make_tag(TagKind::Method, "create_user", 6, 9),
        make_tag(TagKind::Method, "delete_user", 11, 14),
    ];

    let config = OverviewConfig {
        min_methods: 2,
        max_size: 4096,
    };

    let overviews = generate_overview_chunks(source, &tags, &config);

    assert_eq!(overviews.len(), 1);
    let overview = &overviews[0];

    // Should contain class header
    assert!(overview.content.contains("class UserService"));

    // Should contain collapsed method signatures
    assert!(
        overview
            .content
            .contains("fn get_user(id: i64) -> User { ... }")
    );
    assert!(
        overview
            .content
            .contains("fn create_user(name: &str) -> User { ... }")
    );
    assert!(overview.content.contains("fn delete_user(id: i64) { ... }"));

    // Should NOT contain implementation details
    assert!(!overview.content.contains("self.db.find"));
    assert!(!overview.content.contains("User::new"));
}

#[test]
fn test_skip_class_with_few_methods() {
    let source = r#"class SmallClass {
    fn only_method() {}
}"#;

    let tags = vec![
        make_tag(TagKind::Class, "SmallClass", 0, 2),
        make_tag(TagKind::Method, "only_method", 1, 1),
    ];

    let config = OverviewConfig {
        min_methods: 2, // Requires at least 2 methods
        max_size: 4096,
    };

    let overviews = generate_overview_chunks(source, &tags, &config);
    assert!(overviews.is_empty());
}

#[test]
fn test_generate_overview_for_struct_impl() {
    let source = r#"struct Repository {
    db: Database,
}

impl Repository {
    fn find(&self, id: i64) -> Option<Entity> {
        self.db.query(id)
    }

    fn save(&self, entity: &Entity) -> Result<()> {
        self.db.insert(entity)
    }
}"#;

    // Note: We only generate overview for the impl block which contains methods
    let tags = vec![
        make_tag(TagKind::Struct, "Repository", 0, 2),
        make_tag(TagKind::Module, "Repository_impl", 4, 12), // Treat impl as module
        make_tag(TagKind::Method, "find", 5, 7),
        make_tag(TagKind::Method, "save", 9, 11),
    ];

    let config = OverviewConfig::default();
    let overviews = generate_overview_chunks(source, &tags, &config);

    // Should generate overview for the impl block (treated as module)
    assert_eq!(overviews.len(), 1);
}

#[test]
fn test_should_generate_overview() {
    let tags = vec![
        make_tag(TagKind::Class, "MyClass", 0, 20),
        make_tag(TagKind::Method, "method1", 1, 5),
        make_tag(TagKind::Method, "method2", 6, 10),
        make_tag(TagKind::Method, "method3", 11, 15),
    ];

    let container = &tags[0];

    assert!(should_generate_overview(&tags, container, 2));
    assert!(should_generate_overview(&tags, container, 3));
    assert!(!should_generate_overview(&tags, container, 4));
}

#[test]
fn test_collapse_method_to_signature() {
    let source = "    fn process(data: &str) {\n        // do something\n    }";
    let lines: Vec<&str> = source.lines().collect();
    let tag = make_tag(TagKind::Method, "process", 0, 2);

    let result = collapse_method_to_signature(source, &tag, &lines);
    assert!(result.is_some());

    let collapsed = result.unwrap();
    assert!(collapsed.contains("fn process(data: &str) { ... }"));
    assert!(!collapsed.contains("do something"));
}

#[test]
fn test_python_class_overview() {
    let source = r#"class UserManager:
    def __init__(self, db):
        self.db = db

    def get_user(self, user_id):
        return self.db.find(user_id)

    def create_user(self, name, email):
        user = User(name, email)
        self.db.save(user)
        return user
"#;

    let tags = vec![
        make_tag(TagKind::Class, "UserManager", 0, 10),
        make_tag(TagKind::Method, "__init__", 1, 2),
        make_tag(TagKind::Method, "get_user", 4, 5),
        make_tag(TagKind::Method, "create_user", 7, 10),
    ];

    let config = OverviewConfig::default();
    let overviews = generate_overview_chunks(source, &tags, &config);

    assert_eq!(overviews.len(), 1);
    // For Python, we don't have braces, but the function should still work
    // by showing the def lines
}
