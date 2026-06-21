use super::*;

#[test]
fn test_extract_rust() {
    let source = r#"
struct Point {
    x: i32,
    y: i32,
}

fn main() {
    let p = Point { x: 1, y: 2 };
}
"#;
    let mut extractor = SymbolExtractor::new();
    let tags = extractor
        .extract(source, SymbolLanguage::Rust)
        .expect("extract failed");

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Point"), "Should contain Point");
    assert!(names.contains(&"main"), "Should contain main");
}

#[test]
fn test_extract_python() {
    let source = r#"
class Calculator:
    def add(self, a, b):
        return a + b

def main():
    calc = Calculator()
"#;
    let mut extractor = SymbolExtractor::new();
    let tags = extractor
        .extract(source, SymbolLanguage::Python)
        .expect("extract failed");

    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Calculator"));
    assert!(names.contains(&"main"));
}

#[test]
fn test_line_numbers_are_1_indexed() {
    let source = "fn foo() {}\nfn bar() {}\n";
    let mut extractor = SymbolExtractor::new();
    let tags = extractor
        .extract(source, SymbolLanguage::Rust)
        .expect("extract failed");

    let foo = tags
        .iter()
        .find(|t| t.name == "foo")
        .expect("foo not found");
    assert_eq!(foo.line, 1);

    let bar = tags
        .iter()
        .find(|t| t.name == "bar")
        .expect("bar not found");
    assert_eq!(bar.line, 2);
}
