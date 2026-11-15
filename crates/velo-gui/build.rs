fn main() {
    // Debug builds, which the tests are, keep the names and places of the UI's elements, so the
    // flow tests can find a button by its label and click it. Release builds leave them out.
    let debug = std::env::var("PROFILE").is_ok_and(|profile| profile == "debug");
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
