fn main() {
    println!("cargo:rustc-link-arg=/SUBSYSTEM:NATIVE");
    println!("cargo:rustc-link-arg=/DRIVER");
    println!("cargo:rustc-link-arg=/ENTRY:DriverEntry");
    println!("cargo:rustc-link-arg=/NODEFAULTLIB:MSVCRT");
}
