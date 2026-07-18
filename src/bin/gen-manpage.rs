//! Generate man/tess.1 from the clap Args definition. Run with:
//!     cargo run --release --bin gen-manpage
//! Output: man/tess.1 in the repo root.
//!
//! Everything but the GROUPS section is rendered by clap_mangen straight from
//! the clap definitions. The GROUPS section's reserved-names list is built from
//! `tess::format::reserved_group_names()` — the same constant the group loader
//! enforces — so the two can never drift.

use clap::CommandFactory;
use std::fs;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let cmd = tess::cli::Args::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf)?;

    let text = String::from_utf8(buf).expect("clap_mangen output is valid UTF-8");
    let text = insert_groups_section(text);

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let out_dir = PathBuf::from(manifest_dir).join("man");
    fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join("tess.1");
    fs::write(&out_path, text.as_bytes())?;

    println!("wrote {} ({} bytes)", out_path.display(), text.len());
    Ok(())
}

/// Build the roff `.SH GROUPS` section and splice it in before `.SH VERSION`
/// (falling back to appending it if that marker moves in a future clap_mangen).
fn insert_groups_section(text: String) -> String {
    let names: Vec<String> = tess::format::reserved_group_names()
        .iter()
        // Escape the roff hyphen so `chop-long-lines` etc. render literally.
        .map(|n| format!("\\fB{}\\fR", n.replace('-', "\\-")))
        .collect();
    let list = names.join(", ");
    let section = format!(
        ".SH GROUPS\n\
         A CLI group (a \\fB\\-\\-<name>\\fR shortcut defined in \\fBformats.toml\\fR) \
         cannot be named the same as a built\\-in flag. Reserved names: {list}. \
         Attempting to load such a group prints an error and exits.\n"
    );

    match text.find(".SH VERSION") {
        Some(idx) => {
            let mut out = String::with_capacity(text.len() + section.len());
            out.push_str(&text[..idx]);
            out.push_str(&section);
            out.push_str(&text[idx..]);
            out
        }
        None => {
            let mut out = text;
            out.push_str(&section);
            out
        }
    }
}
