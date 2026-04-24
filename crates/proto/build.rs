fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &["proto/remote_desktop.proto"],
            &["proto"],
        )?;
    Ok(())
}
