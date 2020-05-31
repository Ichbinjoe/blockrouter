fn main() {
    println!("cargo:rustc-link-lib=mbedcrypto");
    // TODO(ichbinjoe): #1
    println!("cargo:rustc-link-search=/usr/lib/");
}
