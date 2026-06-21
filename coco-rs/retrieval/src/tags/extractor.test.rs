use super::*;

#[test]
fn test_tag_kind_from_syntax_type() {
    assert_eq!(TagKind::from_syntax_type("function"), TagKind::Function);
    assert_eq!(TagKind::from_syntax_type("method"), TagKind::Method);
    assert_eq!(TagKind::from_syntax_type("class"), TagKind::Class);
    assert_eq!(TagKind::from_syntax_type("struct"), TagKind::Struct);
    assert_eq!(TagKind::from_syntax_type("trait"), TagKind::Interface);
    assert_eq!(TagKind::from_syntax_type("unknown"), TagKind::Other);
}

#[test]
fn test_extract_signature() {
    let source = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}";
    let sig = extract_signature(source, 0, source.len());
    assert_eq!(sig, Some("fn add(a: i32, b: i32) -> i32".to_string()));
}

#[test]
fn test_extract_docs() {
    let source = "/// This is a doc comment\n/// Second line\nfn foo() {}";
    let start = source.find("fn").unwrap();
    let docs = extract_docs(source, start);
    assert!(docs.is_some());
    assert!(docs.unwrap().contains("This is a doc comment"));
}

#[test]
fn test_extract_rust_tags() {
    let source = r#"
/// A simple struct
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

fn main() {
    let p = Point::new(1, 2);
}
"#;
    let mut extractor = TagExtractor::new();
    let tags = extractor.extract(source, SupportedLanguage::Rust).unwrap();

    // Should find: struct Point, fn new, fn main
    assert!(
        tags.len() >= 2,
        "Expected at least 2 tags, got {}",
        tags.len()
    );

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Point"), "Should contain Point struct");
    assert!(names.contains(&"main"), "Should contain main function");
}

#[test]
fn test_extract_go_tags() {
    let source = r#"
package main

type User struct {
    Name string
    Age  int
}

func (u *User) Greet() string {
    return "Hello, " + u.Name
}

func main() {
    u := &User{Name: "Alice", Age: 30}
    fmt.Println(u.Greet())
}
"#;
    let mut extractor = TagExtractor::new();
    let tags = extractor.extract(source, SupportedLanguage::Go).unwrap();

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"User"), "Should contain User struct");
    assert!(names.contains(&"main"), "Should contain main function");
}

#[test]
fn test_extract_python_tags() {
    let source = r#"
class Calculator:
    def add(self, a, b):
        return a + b

    def subtract(self, a, b):
        return a - b

def main():
    calc = Calculator()
    print(calc.add(1, 2))
"#;
    let mut extractor = TagExtractor::new();
    let tags = extractor
        .extract(source, SupportedLanguage::Python)
        .unwrap();

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"Calculator"),
        "Should contain Calculator class"
    );
    assert!(names.contains(&"main"), "Should contain main function");
}

#[test]
fn test_extract_java_tags() {
    let source = r#"
public class HelloWorld {
    private String message;

    public HelloWorld(String msg) {
        this.message = msg;
    }

    public void sayHello() {
        System.out.println(message);
    }

    public static void main(String[] args) {
        HelloWorld hw = new HelloWorld("Hello!");
        hw.sayHello();
    }
}
"#;
    let mut extractor = TagExtractor::new();
    let tags = extractor.extract(source, SupportedLanguage::Java).unwrap();

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"HelloWorld"),
        "Should contain HelloWorld class"
    );
    assert!(names.contains(&"main"), "Should contain main method");
}

#[test]
fn test_find_parent_symbol_class() {
    // Simulate tags from a Python file
    let tags = vec![
        CodeTag {
            name: "Calculator".to_string(),
            kind: TagKind::Class,
            start_line: 1,
            end_line: 10,
            start_byte: 0,
            end_byte: 100,
            signature: None,
            docs: None,
            is_definition: true,
        },
        CodeTag {
            name: "add".to_string(),
            kind: TagKind::Method,
            start_line: 2,
            end_line: 4,
            start_byte: 20,
            end_byte: 50,
            signature: Some("def add(self, a, b)".to_string()),
            docs: None,
            is_definition: true,
        },
    ];

    // Method inside class
    let parent = find_parent_symbol(&tags, 2, 4);
    assert_eq!(parent, Some("class Calculator".to_string()));

    // Top-level function (outside class)
    let parent = find_parent_symbol(&tags, 15, 20);
    assert_eq!(parent, None);
}

#[test]
fn test_find_parent_impl_rust() {
    let source = r#"
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    fn distance(&self) -> f64 {
        ((self.x * self.x + self.y * self.y) as f64).sqrt()
    }
}

fn main() {
    let p = Point::new(3, 4);
}
"#;
    // Line 8 is inside impl Point (fn new)
    let parent = find_parent_impl(source, 8);
    assert!(parent.is_some());
    assert!(parent.unwrap().contains("impl Point"));

    // Line 17 is main function (outside impl)
    let parent = find_parent_impl(source, 17);
    assert!(parent.is_none());
}

#[test]
fn test_get_parent_context() {
    let source = r#"
class UserService:
    def get_user(self, user_id):
        return self.repo.find(user_id)

    def create_user(self, name):
        return self.repo.create(name)

def main():
    service = UserService()
"#;
    let tags = vec![CodeTag {
        name: "UserService".to_string(),
        kind: TagKind::Class,
        start_line: 1,
        end_line: 7,
        start_byte: 0,
        end_byte: 150,
        signature: None,
        docs: None,
        is_definition: true,
    }];

    // Inside class
    let parent = get_parent_context(source, &tags, 2, 4);
    assert_eq!(parent, Some("class UserService".to_string()));

    // Outside class
    let parent = get_parent_context(source, &tags, 9, 10);
    assert_eq!(parent, None);
}
