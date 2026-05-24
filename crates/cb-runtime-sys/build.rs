fn main() {
    cc::Build::new()
        .file("c/catalog.c")
        .include("c")
        .compile("cb_runtime");
}
