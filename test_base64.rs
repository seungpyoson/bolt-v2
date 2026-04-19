use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
fn main() {
    let s = "YWJj\nZGVm";
    match BASE64_STANDARD.decode(s) {
        Ok(_) => println!("Ok"),
        Err(e) => println!("Err: {:?}", e),
    }
}
