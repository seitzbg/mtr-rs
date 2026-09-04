//! Repository tasks for mtr-rs: `cargo xtask man|completions|dist`. GPL-2.0-only.
#![forbid(unsafe_code)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use anyhow::{Context as _, bail};
use clap::{CommandFactory as _, Parser, Subcommand};
use clap_complete::{Generator as _, Shell};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "mtr-rs repository tasks",
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Render mtr-rs.8 (from clap) and copy mtr-rs-packet.8 into --out (default target/dist/man)
    Man {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Generate bash, zsh and fish completions into --out (default target/dist/completions)
    Completions {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Release build + man + completions laid out under target/dist/mtr-rs-<version>-<arch>/
    Dist {
        /// Reuse the existing target/release binaries instead of running cargo build
        #[arg(long)]
        no_build: bool,
    },
}

/// The repository root: xtask/ is a direct child of it.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives directly under the repo root")
        .to_path_buf()
}

/// Workspace version (xtask inherits `version.workspace = true`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Target architecture as Cargo names it (x86_64, aarch64, …).
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// The tarball's top-level directory (and, with `.tar.gz` appended, the tarball's own name).
pub fn dist_dir_name() -> String {
    format!("mtr-rs-{}-{}", version(), arch())
}

/// Repository files copied verbatim into the root of the dist tree. GPL-2.0 §1/§3 require the
/// licence to travel with the binaries, so the tarball is not just bin/man/completions.
pub const DIST_DOCS: [&str; 2] = ["LICENSE", "README.md"];

/// The `.TH` date field: `SOURCE_DATE_EPOCH` (seconds since the Unix epoch, for reproducible
/// builds) if set, else today's UTC date. Either way, deterministic for a given environment.
pub fn man_date() -> String {
    let ts = match std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
    {
        Some(ts) => ts,
        None => jiff::Timestamp::now(),
    };
    ts.to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y-%m-%d")
        .to_string()
}

/// `mtr-rs.8` rendered from the client's clap definition.
pub fn render_man(cmd: clap::Command) -> Vec<u8> {
    // `Args` disables the `--version` flag but clap still needs a version string set on the
    // `Command` for clap_mangen to emit a VERSION section (and fold it into SOURCE).
    let cmd = cmd.version(version());
    let man = clap_mangen::Man::new(cmd)
        .title(mtr::cli::PROGRAM)
        .section("8")
        .date(man_date())
        .source(format!("mtr-rs {}", version()))
        .manual("mtr-rs manual");
    let mut buf = Vec::new();
    man.render(&mut buf).expect("writing to a Vec cannot fail");
    buf
}

/// A completion script for `shell` and the conventional file name for it.
pub fn render_completion(shell: Shell) -> (String, Vec<u8>) {
    let mut cmd = mtr::cli::Args::command();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, mtr::cli::PROGRAM, &mut buf);
    (shell.file_name(mtr::cli::PROGRAM), buf)
}

fn write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("mkdir -p {}", dir.display()))?;
    }
    fs::File::create(path)
        .and_then(|mut f| f.write_all(bytes))
        .with_context(|| format!("write {}", path.display()))
}

fn man(out: &Path) -> anyhow::Result<()> {
    write(
        &out.join("mtr-rs.8"),
        &render_man(mtr::cli::Args::command()),
    )?;
    let helper = repo_root().join("docs/man/mtr-rs-packet.8");
    let text = fs::read(&helper).with_context(|| format!("read {}", helper.display()))?;
    write(&out.join("mtr-rs-packet.8"), &text)?;
    println!("{}", out.display());
    Ok(())
}

fn completions(out: &Path) -> anyhow::Result<()> {
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        let (name, body) = render_completion(shell);
        write(&out.join(name), &body)?;
    }
    println!("{}", out.display());
    Ok(())
}

fn dist(no_build: bool) -> anyhow::Result<()> {
    let root = repo_root();
    if !no_build {
        let status = Process::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--release", "--workspace"])
            .current_dir(&root)
            .status()
            .context("running cargo build --release")?;
        if !status.success() {
            bail!("cargo build --release failed");
        }
    }
    let dist = root.join("target/dist").join(dist_dir_name());
    let _ = fs::remove_dir_all(&dist);
    for bin in ["mtr-rs", "mtr-rs-packet"] {
        let from = root.join("target/release").join(bin);
        let to = dist.join("bin").join(bin);
        fs::create_dir_all(to.parent().unwrap())?;
        fs::copy(&from, &to)
            .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
    }
    for doc in DIST_DOCS {
        let from = root.join(doc);
        let to = dist.join(doc);
        fs::copy(&from, &to)
            .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
    }
    man(&dist.join("man"))?;
    completions(&dist.join("completions"))?;
    // Also mirror into flat, version/arch-independent paths: packaging (cargo-deb's
    // `[package.metadata.deb] assets`) needs stable source paths it can reference from
    // `crates/mtr/Cargo.toml`, whereas `dist` itself is versioned for the tarball layout.
    mirror_flat(&dist.join("man"), &root.join("target/dist/man"))?;
    mirror_flat(
        &dist.join("completions"),
        &root.join("target/dist/completions"),
    )?;
    println!("{}", dist.display());
    Ok(())
}

/// Copy the (flat, non-recursive) contents of `from` into `to`, replacing `to` entirely.
fn mirror_flat(from: &Path, to: &Path) -> anyhow::Result<()> {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).with_context(|| format!("mkdir -p {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("read_dir {}", from.display()))? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        fs::copy(entry.path(), &dest)
            .with_context(|| format!("copy {} -> {}", entry.path().display(), dest.display()))?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let default_out = |sub: &str| repo_root().join("target/dist").join(sub);
    match cli.cmd {
        Cmd::Man { out } => man(&out.unwrap_or_else(|| default_out("man"))),
        Cmd::Completions { out } => completions(&out.unwrap_or_else(|| default_out("completions"))),
        Cmd::Dist { no_build } => dist(no_build),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_man_page_is_section_8_and_lists_the_report_flag() {
        let page = String::from_utf8(render_man(mtr::cli::Args::command())).unwrap();
        // clap_mangen 0.3.3 (pinned via `clap_mangen = "0.3"`) prefixes `render()`'s output with
        // the `roff` crate's fixed apostrophe preamble before the `.TH` control line, so this
        // checks containment rather than a literal prefix; see task-1-report.md.
        assert!(
            page.contains(".TH mtr-rs 8"),
            "{}",
            &page[..page.len().min(160)]
        );
        let th_line = page
            .lines()
            .find(|line| line.starts_with(".TH mtr-rs 8"))
            .expect(".TH line missing");
        let date_field = th_line
            .split_whitespace()
            .nth(3)
            .unwrap_or("")
            .trim_matches('"');
        assert!(
            date_field.len() == 10
                && date_field.as_bytes()[4] == b'-'
                && date_field.as_bytes()[7] == b'-'
                && date_field[..4].bytes().all(|b| b.is_ascii_digit())
                && date_field[5..7].bytes().all(|b| b.is_ascii_digit())
                && date_field[8..10].bytes().all(|b| b.is_ascii_digit()),
            ".TH date field is not YYYY-MM-DD: {th_line:?}"
        );
        assert!(page.contains(".SH NAME"));
        assert!(
            page.contains("\\-\\-report") || page.contains("--report"),
            "options missing"
        );
        assert!(page.contains(version()), "version {} missing", version());
    }

    #[test]
    fn completions_are_named_per_shell_and_mention_the_binary() {
        for (shell, name) in [
            (Shell::Bash, "mtr-rs.bash"),
            (Shell::Zsh, "_mtr-rs"),
            (Shell::Fish, "mtr-rs.fish"),
        ] {
            let (file, body) = render_completion(shell);
            assert_eq!(file, name);
            let body = String::from_utf8(body).unwrap();
            assert!(body.contains("report"), "{name}: no --report completion");
        }
    }

    #[test]
    fn the_helper_man_page_is_shipped_from_docs() {
        let src = repo_root().join("docs/man/mtr-rs-packet.8");
        let text = std::fs::read_to_string(&src).unwrap();
        assert!(text.starts_with(".\\\" GPL-2.0-only"));
        assert!(text.contains(".TH MTR-RS-PACKET 8"));
        assert!(text.contains("send-probe") && text.contains("check-support"));
    }

    #[test]
    fn dist_layout_names_are_stable() {
        assert!(matches!(arch(), "x86_64" | "aarch64" | "arm" | "riscv64"));
        assert_eq!(dist_dir_name(), format!("mtr-rs-{}-{}", version(), arch()));
        assert!(dist_dir_name().starts_with("mtr-rs-"));
    }

    #[test]
    fn the_dist_tree_ships_the_licence_and_readme_next_to_the_binaries() {
        assert_eq!(DIST_DOCS, ["LICENSE", "README.md"]);
        for doc in DIST_DOCS {
            let path = repo_root().join(doc);
            assert!(path.is_file(), "dist would copy a missing {doc}");
        }
        let licence = fs::read_to_string(repo_root().join("LICENSE")).unwrap();
        assert!(
            licence.contains("GNU GENERAL PUBLIC LICENSE"),
            "LICENSE is not the GPL text"
        );
    }

    #[test]
    fn mirror_flat_replaces_the_destination_with_the_sources_contents() {
        let base = std::env::temp_dir().join(format!(
            "mtr-xtask-mirror-flat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let from = base.join("versioned/man");
        let to = base.join("flat/man");
        fs::create_dir_all(&from).unwrap();
        // A pre-existing file under `to` that isn't in `from` must not survive the mirror.
        fs::create_dir_all(&to).unwrap();
        write(&to.join("leftover.8"), b"leftover").unwrap();
        write(&from.join("mtr-rs.8"), b"man page body").unwrap();
        write(&from.join("mtr-rs-packet.8"), b"helper man page body").unwrap();

        mirror_flat(&from, &to).unwrap();

        assert_eq!(
            fs::read_to_string(to.join("mtr-rs.8")).unwrap(),
            "man page body"
        );
        assert_eq!(
            fs::read_to_string(to.join("mtr-rs-packet.8")).unwrap(),
            "helper man page body"
        );
        assert!(!to.join("leftover.8").exists(), "stale file not removed");

        fs::remove_dir_all(&base).unwrap();
    }
}
