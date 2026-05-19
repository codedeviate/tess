//! Generate man/tess.1 from the clap Args definition. Run with:
//!     cargo run --release --bin gen-manpage
//! Output: man/tess.1 in the repo root.

use clap::CommandFactory;
use std::fs;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let cmd = tess::cli::Args::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf)?;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".into());
    let out_dir = PathBuf::from(manifest_dir).join("man");
    fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join("tess.1");
    fs::write(&out_path, &buf)?;

    println!("wrote {} ({} bytes)", out_path.display(), buf.len());
    Ok(())
}
