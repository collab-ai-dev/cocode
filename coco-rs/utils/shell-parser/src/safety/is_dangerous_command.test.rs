use super::*;

fn vec_str(items: &[&str]) -> Vec<String> {
    items.iter().map(std::string::ToString::to_string).collect()
}

#[test]
fn rm_rf_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&["rm", "-rf", "/"])));
}

#[test]
fn rm_f_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&["rm", "-f", "/"])));
}

#[test]
fn ls_is_not_dangerous() {
    assert!(!command_might_be_dangerous(&vec_str(&["ls", "-la"])));
}

#[test]
fn sudo_rm_rf_is_dangerous() {
    assert!(command_might_be_dangerous(&vec_str(&[
        "sudo", "rm", "-rf", "/"
    ])));
}
