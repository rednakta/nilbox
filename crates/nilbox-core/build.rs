fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/keystore/provider/biometric_helper.m")
            .flag("-fmodules")
            .compile("biometric_helper");

        println!("cargo:rustc-link-lib=framework=LocalAuthentication");
    }
}
