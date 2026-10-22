//! Print a read-only inspection report of the local DaVinci Resolve install.
//! `cargo run -p pirosman-core --example resolve_inspect`
fn main() {
    let r = pirosman_core::resolve_re::inspect_default();
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
}
